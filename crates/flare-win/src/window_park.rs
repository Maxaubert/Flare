//! Parks the --e2e window off every monitor, without activating it (Task 8).

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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

/// What the harness checks of an `--e2e` window: where it is, how it is styled, whether it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkReport {
    /// Left, top, right, bottom in device pixels.
    pub rect: (i32, i32, i32, i32),
    /// The extended window style.
    pub ex_style: u32,
    /// The virtual screen (all monitors): left, top, right, bottom.
    pub screen: (i32, i32, i32, i32),
    /// Whether this window is the foreground window.
    pub foreground: bool,
}

/// Pure: whether two rectangles (left, top, right, bottom) share any pixel.
pub fn rects_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

impl ParkReport {
    /// Whether any part of the window lies on any monitor.
    pub fn on_screen(&self) -> bool {
        rects_overlap(self.rect, self.screen)
    }

    /// Everything wrong with the window as an `--e2e` window; empty when it is safe.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.on_screen() {
            out.push(format!(
                "the window is on a monitor at {:?} (screen {:?}), it is not parked",
                self.rect, self.screen
            ));
        }
        if self.ex_style & WS_EX_NOACTIVATE == 0 {
            out.push("WS_EX_NOACTIVATE is not set".into());
        }
        if self.ex_style & WS_EX_TOOLWINDOW == 0 {
            out.push("WS_EX_TOOLWINDOW is not set".into());
        }
        if self.ex_style & WS_EX_APPWINDOW != 0 {
            out.push("WS_EX_APPWINDOW is set, the window would show on the taskbar".into());
        }
        if self.foreground {
            out.push("the window is the foreground window".into());
        }
        out
    }
}

/// Reads the report of a live window.
pub fn report(hwnd: isize) -> ParkReport {
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: plain window-manager reads on a handle the caller owns; a stale handle just fails
    // and leaves the zeroed rectangle. `r` is a valid out pointer.
    let (ex_style, foreground, screen) = unsafe {
        GetWindowRect(h, &mut r);
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (
            GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
            GetForegroundWindow() == h,
            (
                x,
                y,
                x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
                y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ),
        )
    };
    ParkReport {
        rect: (r.left, r.top, r.right, r.bottom),
        ex_style,
        screen,
        foreground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32, i32, i32) = (0, 0, 1920, 1080);

    fn safe() -> ParkReport {
        ParkReport {
            rect: (-32000, -32000, -31000, -31500),
            ex_style: WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            screen: SCREEN,
            foreground: false,
        }
    }

    #[test]
    fn a_parked_styled_background_window_has_no_problems() {
        assert!(safe().problems().is_empty());
    }

    #[test]
    fn a_window_on_a_monitor_is_a_problem_even_if_only_partly() {
        let mut r = safe();
        r.rect = (1900, 100, 2900, 700);
        assert!(r.on_screen());
        assert!(r.problems().iter().any(|p| p.contains("not parked")));
        // Left of a second monitor at a negative origin counts too.
        r.screen = (-1920, 0, 1920, 1080);
        r.rect = (-1000, 0, -10, 500);
        assert!(r.on_screen());
    }

    #[test]
    fn the_style_must_be_no_activate_tool_window_and_not_app_window() {
        let mut r = safe();
        r.ex_style = 0;
        assert_eq!(r.problems().len(), 2);
        r.ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert_eq!(r.problems().len(), 1);
    }

    #[test]
    fn the_foreground_window_is_a_problem() {
        let mut r = safe();
        r.foreground = true;
        assert_eq!(r.problems().len(), 1);
    }

    #[test]
    fn a_live_window_parked_reports_off_every_monitor() {
        let hwnd = hidden_popup();
        park(hwnd);
        let rep = report(hwnd);
        // SAFETY: the handle was just created here.
        unsafe { DestroyWindow(hwnd as HWND) };
        assert_eq!((rep.rect.0, rep.rect.1), (-32000, -32000));
        assert!(!rep.on_screen());
        assert!(rep.screen.2 > rep.screen.0);
    }

    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};

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
