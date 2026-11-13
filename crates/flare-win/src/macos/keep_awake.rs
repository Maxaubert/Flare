//! macOS keep-awake (remote sessions PR 4, rule 296): one IOKit power assertion of the kind
//! `PreventUserIdleSystemSleep` while a remote session works. The system ends the assertion with the process too, so a
//! crash never leaves the Mac awake. Only idle system sleep is held off: the display still sleeps and the screen still
//! locks.

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_foundation::NSString;

/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;
/// `kIOPMAssertPreventUserIdleSystemSleep`.
const PREVENT_IDLE_SLEEP: &str = "PreventUserIdleSystemSleep";
/// What `pmset -g assertions` shows for it.
const REASON: &str = "Flare: a remote session is working";

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: *const c_void,
        level: u32,
        name: *const c_void,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

/// The one assertion, while held.
pub struct Awake {
    id: Option<u32>,
}

impl Awake {
    pub fn new() -> Self {
        Self { id: None }
    }
}

impl super::SystemSleep for Awake {
    fn set(&mut self, awake: bool) {
        match (awake, self.id) {
            (true, None) => {
                let kind: Retained<NSString> = NSString::from_str(PREVENT_IDLE_SLEEP);
                let name: Retained<NSString> = NSString::from_str(REASON);
                let mut id: u32 = 0;
                // SAFETY: an NSString is a CFString (toll-free bridged), both live across the call; `id` is a valid out
                // pointer.
                let st = unsafe {
                    IOPMAssertionCreateWithName(
                        Retained::as_ptr(&kind).cast(),
                        LEVEL_ON,
                        Retained::as_ptr(&name).cast(),
                        &mut id,
                    )
                };
                if st == 0 {
                    self.id = Some(id);
                } else {
                    log::warn!("keep-awake: the power assertion was refused ({st})");
                }
            }
            (false, Some(id)) => {
                // SAFETY: `id` came from a successful create and is released once.
                let st = unsafe { IOPMAssertionRelease(id) };
                if st != 0 {
                    log::warn!("keep-awake: the power assertion was not released ({st})");
                }
                self.id = None;
            }
            _ => {}
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        super::SystemSleep::set(self, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keep_awake::{KeepAwake, SystemAwake};
    use std::time::{Duration, Instant};

    /// The lines of `pmset -g assertions` that are this process's keep-awake.
    fn ours() -> Vec<String> {
        let out = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("pmset runs");
        let me = format!("pid {}(", std::process::id());
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains(&me) && l.contains(PREVENT_IDLE_SLEEP) && l.contains(REASON))
            .map(str::to_string)
            .collect()
    }

    /// Waits (briefly) for the power manager's list to say `held`.
    fn listed(held: bool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if ours().is_empty() != held {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Rule 296 on a real Mac: the first hold makes one real `PreventUserIdleSystemSleep` assertion of this process,
    /// named for Flare (what `pmset -g assertions` shows the owner), a second hold makes no other, and the last drop
    /// releases it. Held for well under a second: only idle sleep is put off, and only while the test runs.
    #[test]
    fn a_hold_makes_one_real_assertion_and_its_last_drop_releases_it() {
        let _turn = crate::keep_awake::REAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(
            listed(false),
            "no assertion of this process before the hold"
        );
        let awake = SystemAwake::new();
        let a = awake.hold();
        assert!(listed(true), "the hold is listed by pmset");
        let b = awake.hold();
        assert_eq!(ours().len(), 1, "two holds are one assertion: {:?}", ours());
        drop(a);
        assert_eq!(ours().len(), 1, "one hold left keeps it");
        drop(b);
        assert!(listed(false), "the last drop releases it: {:?}", ours());

        // The drop of the system's side releases a held assertion too (the window's end).
        let mut a = Awake::new();
        crate::keep_awake::SystemSleep::set(&mut a, true);
        assert!(a.id.is_some(), "the assertion was made");
        assert!(listed(true));
        drop(a);
        assert!(listed(false), "released on drop: {:?}", ours());
    }
}
