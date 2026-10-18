//! Parks the --e2e window off every monitor, without activating it (Task 8).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

/// Where a parked window sits: far outside every monitor's rectangle.
const PARK_AT: i32 = -32000;

/// The flags of the park call: keep the size and the z-order, and never activate.
pub const PARK_FLAGS: u32 = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;

/// The top-left corner of a parked window, in device pixels.
pub fn parked_origin() -> (i32, i32) {
    (PARK_AT, PARK_AT)
}

/// Moves the window to the parking spot. Never activates it, never resizes it.
pub fn park(hwnd: isize) {
    let (x, y) = parked_origin();
    // SAFETY: a plain window-manager call on a handle the caller owns; a stale handle just fails.
    unsafe {
        SetWindowPos(hwnd as HWND, std::ptr::null_mut(), x, y, 0, 0, PARK_FLAGS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WS_POPUP,
    };

    fn hidden_popup() -> isize {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: the system STATIC class, a hidden popup of 100 by 100 at the origin.
        let h = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                100,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!h.is_null(), "the test window was not created");
        h as isize
    }

    #[test]
    fn park_moves_the_window_off_every_monitor() {
        let hwnd = hidden_popup();
        park(hwnd);
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: the handle was just created, `r` is a valid out pointer.
        unsafe {
            GetWindowRect(hwnd as HWND, &mut r);
            DestroyWindow(hwnd as HWND);
        }
        assert_eq!((r.left, r.top), (-32000, -32000));
        assert_eq!(parked_origin(), (-32000, -32000));
        assert_eq!((r.right - r.left, r.bottom - r.top), (100, 100));
    }

    #[test]
    fn parking_never_activates() {
        assert_ne!(PARK_FLAGS & SWP_NOACTIVATE, 0);
        assert_ne!(PARK_FLAGS & SWP_NOSIZE, 0);
        assert_ne!(PARK_FLAGS & SWP_NOZORDER, 0);
    }
}
