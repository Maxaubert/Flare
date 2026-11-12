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
