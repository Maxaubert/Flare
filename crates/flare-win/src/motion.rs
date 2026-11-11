//! Whether the system wants animation: the native `prefers-reduced-motion`, read on Windows from the
//! "Animation effects" switch under Settings > Accessibility > Visual effects (Task 21), and on a Mac from
//! "Reduce motion" under System Settings > Accessibility > Display (regression rule 1135).
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

/// Whether the system lets the window animate: Windows' "Animation effects" switch is on, or a Mac's "Reduce motion"
/// is off. Any failure answers `true`.
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
    #[cfg(target_os = "macos")]
    {
        // sync-ok: one NSWorkspace property read (any thread), returns at once
        let reduce =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        animations_from(true, i32::from(!reduce))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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

    /// Rule 1135: on a Mac the answer is "Reduce motion" itself, read off the main thread too (the agent icons and
    /// every mark stand still when it is on, as with Windows' switch off).
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_reduce_motion_decides() {
        let reduce = std::thread::spawn(|| {
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
        })
        .join()
        .unwrap();
        assert_eq!(animations_enabled(), !reduce);
    }

    #[test]
    fn the_real_call_answers_without_panicking() {
        let on = animations_enabled();
        println!("animations_enabled() = {on}");
    }
}
