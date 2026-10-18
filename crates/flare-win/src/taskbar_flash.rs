//! Flashes the taskbar button when a bell rings in a window nobody is looking at (regression rule 20):
//! no sound, only the button, and only while the window is not the foreground one. The OS stops the
//! flash by itself when the window comes to the front.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetForegroundWindow,
};

/// What the flash call is asked: the taskbar button only (never the caption), until the window is
/// in front.
pub fn flash_info(hwnd: isize) -> FLASHWINFO {
    FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: hwnd as HWND,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    }
}

/// True when `hwnd` is the foreground window.
pub fn is_foreground(hwnd: isize) -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { GetForegroundWindow() as isize == hwnd }
}

/// Flashes the taskbar button of `hwnd` unless that window is the foreground one. True when it
/// flashed.
pub fn flash_unless_foreground(hwnd: isize) -> bool {
    if is_foreground(hwnd) {
        return false;
    }
    let info = flash_info(hwnd);
    // SAFETY: `info` is a valid FLASHWINFO for the call; a stale handle just fails.
    unsafe {
        FlashWindowEx(&info);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flash_is_the_tray_button_until_the_window_is_in_front() {
        let i = flash_info(1234);
        assert_eq!(i.dwFlags, FLASHW_TRAY | FLASHW_TIMERNOFG);
        assert_eq!(i.hwnd as isize, 1234);
        assert_eq!(
            i.uCount, 0,
            "no count: it runs until the window is in front"
        );
        assert_eq!(i.cbSize as usize, size_of::<FLASHWINFO>());
    }

    #[test]
    fn a_window_that_does_not_exist_is_not_in_front() {
        assert!(!is_foreground(-4242));
    }
}
