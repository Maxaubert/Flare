//! DwmSetWindowAttribute wrappers (Task 18).
//!
//! Port of Prism Terminal's `dwmHelper.ts`, now in process (no PowerShell child). Every wrapper
//! answers `true` when the HRESULT is a success and never panics: the calls are cosmetic
//! (rule 74), a failure leaves the window as it was.

/// A native window handle as an integer; converted to `HWND` at the call.
pub type Hwnd = isize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Border {
    None,
    Default,
    /// `#rrggbb`
    Colour(String),
}

/// `0x00BBGGRR`; a leading `#` is optional. Junk reads as black.
pub fn colorref_of(hex: &str) -> u32 {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0) & 0x00ff_ffff;
    let (r, g, b) = ((n >> 16) & 255, (n >> 8) & 255, n & 255);
    (b << 16) | (g << 8) | r
}

#[cfg(windows)]
mod native {
    use super::Hwnd;
    use core::ffi::c_void;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute};

    pub fn set_attr<T>(hwnd: Hwnd, attr: i32, value: &T) -> bool {
        // sync-ok: DWM call, returns in microseconds
        // SAFETY: `value` points at a live T of the size DWM expects for this attribute; a bad
        // handle makes the call fail with an HRESULT, not crash.
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                attr as u32,
                value as *const T as *const c_void,
                size_of::<T>() as u32,
            )
        };
        hr >= 0
    }

    pub fn get_i32(hwnd: Hwnd, attr: u32) -> Option<i32> {
        let mut v: i32 = 0;
        // sync-ok: DWM call, returns in microseconds
        // SAFETY: `v` is a valid, writable i32 and the size passed is its size.
        let hr = unsafe {
            DwmGetWindowAttribute(hwnd as HWND, attr, &mut v as *mut i32 as *mut c_void, 4)
        };
        (hr >= 0).then_some(v)
    }
}

#[cfg(not(windows))]
mod native {
    use super::Hwnd;
    pub fn set_attr<T>(_: Hwnd, _: i32, _: &T) -> bool {
        false
    }
    pub fn get_i32(_: Hwnd, _: u32) -> Option<i32> {
        None
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
const DWMWA_USE_IMMERSIVE_DARK_MODE: i32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: i32 = 33;
const DWMWA_BORDER_COLOR: i32 = 34;
const DWMWA_COLOR_NONE: u32 = 4294967294;
const DWMWA_COLOR_DEFAULT: u32 = 4294967295;

pub fn set_border(hwnd: Hwnd, b: &Border) -> bool {
    let c: u32 = match b {
        Border::None => DWMWA_COLOR_NONE,
        Border::Default => DWMWA_COLOR_DEFAULT,
        Border::Colour(hex) => colorref_of(hex),
    };
    native::set_attr(hwnd, DWMWA_BORDER_COLOR, &c)
}

/// The window's dark or light frame, following the theme's ground. macOS: the window wears the dark or
/// the light Aqua appearance, so the traffic lights follow the ground (`ns_window.rs`; `hwnd` is the
/// window's `NSView` there).
pub fn set_immersive_dark(hwnd: Hwnd, dark: bool) -> bool {
    #[cfg(target_os = "macos")]
    return crate::ns_window::set_dark(hwnd, dark);
    #[cfg(not(target_os = "macos"))]
    native::set_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &i32::from(dark))
}

/// DWMWCP_DEFAULT (0) when rounded, DWMWCP_DONOTROUND (1) otherwise.
pub fn set_corners_rounded(hwnd: Hwnd, rounded: bool) -> bool {
    let v: i32 = if rounded { 0 } else { 1 };
    native::set_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &v)
}

/// For tests and the diagnostics log.
pub fn read_back_i32(hwnd: Hwnd, attr: u32) -> Option<i32> {
    native::get_i32(hwnd, attr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_has_red_in_the_low_byte() {
        assert_eq!(colorref_of("#ff0000"), 0x0000ff);
        assert_eq!(colorref_of("#0000ff"), 0xff0000);
        assert_eq!(colorref_of("#34373d"), 0x3d3734);
        assert_eq!(colorref_of("c9ccd3"), 0xd3ccc9);
    }

    #[test]
    fn the_dwm_wrappers_do_not_panic_on_a_bad_handle() {
        assert!(!set_border(0, &Border::Colour("#112233".into())));
        assert!(!set_border(0, &Border::None));
        assert!(!set_immersive_dark(0, true));
        assert!(!set_corners_rounded(0, false));
        assert_eq!(read_back_i32(0, 34), None);
    }
}
