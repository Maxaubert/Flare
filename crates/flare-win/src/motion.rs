//! Whether Windows wants animation: the native `prefers-reduced-motion`, read from the
//! "Animation effects" switch under Settings > Accessibility > Visual effects (Task 21).
//!
//! The answer is read once per decision; a failed call answers `true` (animate), so a machine
//! that cannot say never loses its motion.

/// `SPI_GETCLIENTAREAANIMATION`: the system switch for animation inside client areas.
#[cfg(windows)]
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

/// Pure rule: a failed call means animate; otherwise the switch is on when its value is non-zero.
pub fn animations_from(call_ok: bool, value: i32) -> bool {
    if !call_ok {
        return true;
    }
    value != 0
}

/// Whether Windows' "Animation effects" switch is on. Any failure answers `true`.
pub fn animations_enabled() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
        let mut value: i32 = 1;
        // sync-ok: a SystemParametersInfo read of one flag, returns at once
        // SAFETY: `value` is a live i32 for the whole call, which SPI_GETCLIENTAREAANIMATION
        // writes as a BOOL (4 bytes); the call takes no other pointer.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                &mut value as *mut i32 as *mut core::ffi::c_void,
                0,
            )
        };
        animations_from(ok != 0, value)
    }
    #[cfg(not(windows))]
    {
        animations_from(false, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_call_means_animate() {
        assert!(animations_from(false, 0));
    }

    #[test]
    fn the_setting_decides() {
        assert!(!animations_from(true, 0));
        assert!(animations_from(true, 1));
        assert!(animations_from(true, 7));
    }

    #[test]
    fn the_real_call_answers_without_panicking() {
        let on = animations_enabled();
        println!("animations_enabled() = {on}");
    }
}
