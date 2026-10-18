//! The Alt+Space system menu on the frameless window (Task 18).
//!
//! Flare's window has no frame, so Windows does not give it the system menu: the style gets
//! `WS_SYSMENU` (and the two box bits) without a caption, Alt+Space shows the menu at the window's
//! top-left as it does for a captioned window, and a right click shows none.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetSystemMenu, GetWindowLongPtrW, GetWindowRect, PostMessageW, SetForegroundWindow,
    SetWindowLongPtrW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, TrackPopupMenu, WM_CONTEXTMENU,
    WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WM_SYSCOMMAND, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
};

/// What the menu needs from the OS, so a test can stand in for it.
pub trait SystemMenu {
    /// Shows the menu at the screen point and returns the chosen command (0: none).
    fn track(&self, hwnd: isize, x_px: i32, y_px: i32) -> u32;
    /// Posts the chosen command to the window as a `WM_SYSCOMMAND`.
    fn post_syscommand(&self, hwnd: isize, cmd: u32);
}

/// The real thing: `GetSystemMenu` and `TrackPopupMenu`.
pub struct WinSystemMenu;

impl SystemMenu for WinSystemMenu {
    fn track(&self, hwnd: isize, x_px: i32, y_px: i32) -> u32 {
        let h = hwnd as HWND;
        // SAFETY: plain Win32 calls on a window of this process; the menu belongs to the window.
        unsafe {
            let menu = GetSystemMenu(h, 0);
            if menu.is_null() {
                return 0;
            }
            // The menu closes on a click elsewhere only when its owner is the foreground window.
            SetForegroundWindow(h);
            // sync-ok: the menu's own modal loop, opened by the user's Alt+Space and closed by
            // their next click or key.
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
                x_px,
                y_px,
                0,
                h,
                std::ptr::null(),
            ) as u32
        }
    }

    fn post_syscommand(&self, hwnd: isize, cmd: u32) {
        // SAFETY: posting a message to a window of this process.
        unsafe {
            PostMessageW(hwnd as HWND, WM_SYSCOMMAND, cmd as usize, 0);
        }
    }
}

/// Shows the menu through `port` and posts the chosen command. Returns the command (0: none).
pub fn show_with(port: &dyn SystemMenu, hwnd: isize, x_px: i32, y_px: i32) -> u32 {
    let cmd = port.track(hwnd, x_px, y_px);
    if cmd != 0 {
        port.post_syscommand(hwnd, cmd);
    }
    cmd
}

/// Shows the real system menu at the screen point and posts the chosen command as a
/// `WM_SYSCOMMAND` (needs `WS_SYSMENU` on the style, see [`ensure_sysmenu_style`]).
pub fn show_system_menu(hwnd: isize, x_px: i32, y_px: i32) {
    show_with(&WinSystemMenu, hwnd, x_px, y_px);
}

/// Shows the real menu at the window's top-left corner, as Windows does for Alt+Space.
pub fn show_at_top_left(hwnd: isize) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: reads the rectangle of a window of this process into a local.
    let ok = unsafe { GetWindowRect(hwnd as HWND, &mut r) };
    if ok != 0 {
        show_system_menu(hwnd, r.left, r.top);
    }
}

/// The style with the system menu and the two box bits added; a caption is never added.
pub fn with_sysmenu_style(style: u32) -> u32 {
    style | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX
}

/// Adds `WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX` to a frameless window, without a caption.
pub fn ensure_sysmenu_style(hwnd: isize) {
    let h = hwnd as HWND;
    // SAFETY: reads and writes the style of a window of this process.
    unsafe {
        let cur = GetWindowLongPtrW(h, GWL_STYLE);
        let new = with_sysmenu_style(cur as u32);
        if new as isize != cur {
            SetWindowLongPtrW(h, GWL_STYLE, new as isize);
        }
    }
}

/// True for the messages that would open the native menu on a right click: the default handler
/// must not see them.
pub fn refuse_context(msg: u32) -> bool {
    matches!(msg, WM_NCRBUTTONUP | WM_NCRBUTTONDOWN | WM_CONTEXTMENU)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SC_CLOSE, WS_CAPTION, WS_POPUP,
    };

    struct Fake {
        pick: u32,
        tracked: RefCell<Vec<(isize, i32, i32)>>,
        posted: RefCell<Vec<(isize, u32)>>,
    }

    impl SystemMenu for Fake {
        fn track(&self, hwnd: isize, x: i32, y: i32) -> u32 {
            self.tracked.borrow_mut().push((hwnd, x, y));
            self.pick
        }
        fn post_syscommand(&self, hwnd: isize, cmd: u32) {
            self.posted.borrow_mut().push((hwnd, cmd));
        }
    }

    fn fake(pick: u32) -> Fake {
        Fake {
            pick,
            tracked: RefCell::default(),
            posted: RefCell::default(),
        }
    }

    #[test]
    fn the_menu_commands_are_posted_as_syscommand() {
        let f = fake(SC_CLOSE);
        assert_eq!(show_with(&f, 7, 10, 20), SC_CLOSE);
        assert_eq!(*f.tracked.borrow(), vec![(7, 10, 20)]);
        assert_eq!(*f.posted.borrow(), vec![(7, SC_CLOSE)]);
    }

    #[test]
    fn a_dismissed_menu_posts_nothing() {
        let f = fake(0);
        show_with(&f, 7, 0, 0);
        assert!(f.posted.borrow().is_empty());
    }

    #[test]
    fn a_right_click_on_the_caption_is_refused() {
        assert!(refuse_context(WM_NCRBUTTONUP));
        assert!(refuse_context(WM_CONTEXTMENU));
        assert!(!refuse_context(WM_SYSCOMMAND));
    }

    #[test]
    fn the_style_gets_sysmenu_without_a_caption() {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: makes, reads and destroys a hidden popup window on this thread.
        unsafe {
            let h = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!h.is_null());
            ensure_sysmenu_style(h as isize);
            let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
            assert_ne!(style & WS_SYSMENU, 0);
            assert_ne!(style & WS_MINIMIZEBOX, 0);
            assert_ne!(style & WS_MAXIMIZEBOX, 0);
            assert_ne!(style & WS_POPUP, 0);
            assert_ne!(style & WS_CAPTION, WS_CAPTION);
            assert_eq!(style & WS_CAPTION, 0);
            DestroyWindow(h);
        }
    }
}
