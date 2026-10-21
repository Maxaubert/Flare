//! Native resize edges and corners on the frameless main window.
//!
//! Slint's `no-frame` window is a winit undecorated window: winit keeps `WS_SIZEBOX` on it (so Aero
//! Snap and the snap layouts work) but answers `WM_NCCALCSIZE` with the whole window as client area,
//! so there is no non-client border and `DefWindowProc` answers `WM_NCHITTEST` with `HTCLIENT`
//! everywhere: no handles, no resize cursors. Prism Terminal (Electron frameless) resizes from
//! invisible edges; this subclass gives Flare the same behaviour (not the same pixel sizes) without any visible frame. It answers
//! `WM_NCHITTEST` with `HTLEFT`..`HTBOTTOMRIGHT` inside a band along the window's edge and passes
//! everything else on, so the title-bar drag, the window buttons, the tab strip and the terminal are
//! untouched. `DefWindowProc` then supplies the resize cursors and the sizing loop, and
//! `WM_GETMINMAXINFO` (winit, from Slint's `min-width`/`min-height`, 560 x 400 as Prism Terminal)
//! keeps the minimum.
//!
//! The band is `SM_CXSIZEFRAME + SM_CXPADDEDBORDER` at the window's own DPI (8 px at 100 percent,
//! the size Windows gives a sizable window's border), read on every hit test, so a `WM_DPICHANGED` needs
//! no work. A corner reaches twice as far along each edge as the edge is thick. No band while
//! maximized, minimized or full screen.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, IsIconic, IsZoomed, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_NOSIZE,
    WINDOWPOS, WM_NCDESTROY, WM_NCHITTEST, WM_WINDOWPOSCHANGING,
};

/// The subclass id (one per window).
const RESIZE_ID: usize = 0x464C_5253;

/// The thickness of the resize band, in device px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Left, right and bottom edges, and the top edge.
    pub x: i32,
    pub y: i32,
}

impl Band {
    /// The band for the frame metrics (`SM_C?SIZEFRAME` plus `SM_CXPADDEDBORDER`), at least 1 px.
    pub fn from_metrics(frame_x: i32, frame_y: i32, padded: i32) -> Self {
        Self {
            x: (frame_x + padded).max(1),
            y: (frame_y + padded).max(1),
        }
    }

    /// The band at a DPI from the metrics Windows gives for it.
    pub fn at_dpi(dpi: u32) -> Self {
        // SAFETY: plain metric reads.
        let (fx, fy, p) = unsafe {
            (
                GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi),
                GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi),
                GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi),
            )
        };
        Self::from_metrics(fx, fy, p)
    }
}

/// Which handle a point of the window is, as the `HT*` code `WM_NCHITTEST` answers.
/// `x` and `y` are relative to the window's top-left, `w` and `h` its size, all in device px.
/// `None` is the rest of the window (the client).
pub fn hit_test(w: i32, h: i32, x: i32, y: i32, band: Band) -> Option<u32> {
    if x < 0 || y < 0 || x >= w || y >= h {
        return None;
    }
    // A corner reaches twice as far as the edge is thick, but never past the middle.
    let (bx, by) = (band.x.min(w / 2), band.y.min(h / 2));
    let cx = (band.x * 2).min(w / 2);
    let cy = (band.y * 2).min(h / 2);
    let left = x < bx;
    let right = x >= w - bx;
    let top = y < by;
    let bottom = y >= h - by;
    if !(left || right || top || bottom) {
        return None;
    }
    let near_left = x < cx;
    let near_right = x >= w - cx;
    let near_top = y < cy;
    let near_bottom = y >= h - cy;
    Some(match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        _ if left => HTLEFT,
        _ if right => HTRIGHT,
        _ if top => HTTOP,
        _ => HTBOTTOM,
    })
}

/// True for a window that is not a plain sized window: maximized, minimized, or exactly covering its
/// monitor (full screen). It has no resize band.
pub fn is_flat(zoomed: bool, iconic: bool, rect: RECT, monitor: RECT) -> bool {
    zoomed
        || iconic
        || (rect.left == monitor.left
            && rect.top == monitor.top
            && rect.right == monitor.right
            && rect.bottom == monitor.bottom)
}

/// The hit test of a real window for a screen point: `Some(HT code)` in a resize band, else `None`.
pub fn hit_test_window(hwnd: isize, sx: i32, sy: i32) -> Option<u32> {
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: reads of a window of this process into locals.
    unsafe {
        if GetWindowRect(h, &mut r) == 0 {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let monitor = if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 {
            mi.rcMonitor
        } else {
            RECT {
                left: i32::MIN,
                top: i32::MIN,
                right: i32::MIN,
                bottom: i32::MIN,
            }
        };
        if is_flat(IsZoomed(h) != 0, IsIconic(h) != 0, r, monitor) {
            return None;
        }
        let dpi = match GetDpiForWindow(h) {
            0 => 96,
            d => d,
        };
        hit_test(
            r.right - r.left,
            r.bottom - r.top,
            sx - r.left,
            sy - r.top,
            Band::at_dpi(dpi),
        )
    }
}

unsafe extern "system" fn resize_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        // The point is in screen coordinates, signed 16 bit each (negative on a monitor left of the primary).
        let sx = i32::from(lparam as i16);
        let sy = i32::from((lparam >> 16) as i16);
        if let Some(code) = hit_test_window(hwnd as isize, sx, sy) {
            return code as LRESULT;
        }
    }
    if msg == WM_NCDESTROY {
        // SAFETY: removes this subclass before the window goes, as comctl32 asks.
        unsafe { RemoveWindowSubclass(hwnd, Some(resize_proc), RESIZE_ID) };
    }
    if msg == WM_WINDOWPOSCHANGING {
        // SAFETY: for this message `lparam` points at the WINDOWPOS the system is about to apply.
        let pos = unsafe { &mut *(lparam as *mut WINDOWPOS) };
        if pos.flags & SWP_NOSIZE == 0
            // SAFETY: reads the state of a window of this process.
            && unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 }
            && !covers_monitor(hwnd, pos)
        {
            let min = min_track(hwnd as isize);
            let (cx, cy) = clamp_size(pos.cx, pos.cy, min);
            pos.cx = cx;
            pos.cy = cy;
        }
    }
    // SAFETY: the next procedure in the chain, with the arguments this one was given.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// True when the incoming position is exactly the monitor's rectangle (borderless full screen),
/// which must not be pushed past the monitor by the minimum.
fn covers_monitor(hwnd: HWND, pos: &WINDOWPOS) -> bool {
    // SAFETY: reads the monitor of a window of this process into locals.
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi) == 0 {
            return false;
        }
        is_rect(pos.x, pos.y, pos.cx, pos.cy, mi.rcMonitor)
    }
}

/// True when the position and size are exactly the rectangle.
pub fn is_rect(x: i32, y: i32, cx: i32, cy: i32, r: RECT) -> bool {
    x == r.left && y == r.top && x + cx == r.right && y + cy == r.bottom
}

/// A window size held to the minimum: Windows applies `ptMinTrackSize` to a drag only, so a
/// programmatic resize (a restore, a script) is held to it here.
pub fn clamp_size(cx: i32, cy: i32, min: (i32, i32)) -> (i32, i32) {
    (cx.max(min.0), cy.max(min.1))
}

/// `ptMinTrackSize` as the window procedure answers `WM_GETMINMAXINFO`.
pub fn min_track(hwnd: isize) -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, SendMessageW, WM_GETMINMAXINFO};
    // SAFETY: a message to a window of this process (from its own thread), with a local to fill.
    unsafe {
        let mut mmi: MINMAXINFO = std::mem::zeroed();
        mmi.ptMinTrackSize = POINT { x: 0, y: 0 };
        SendMessageW(
            hwnd as HWND,
            WM_GETMINMAXINFO,
            0,
            &mut mmi as *mut MINMAXINFO as LPARAM,
        );
        (mmi.ptMinTrackSize.x, mmi.ptMinTrackSize.y)
    }
}

/// Puts the resize edges on the main window, on its own thread. Safe to call again (one subclass).
pub fn install(hwnd: isize) -> bool {
    // SAFETY: subclasses a window of this process from its own thread; the procedure is a plain function.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(resize_proc), RESIZE_ID, 0) != 0 }
}

// ---- what the e2e reads through the real window procedure ---------------------------------------

/// What the window says about its resize edges, read through its window procedure (the e2e probe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeReport {
    /// The window rectangle `[left, top, right, bottom]` in device px.
    pub rect: [i32; 4],
    pub dpi: u32,
    /// The band thickness in device px (edge, top edge).
    pub band: (i32, i32),
    /// The window carries `WS_SIZEBOX` (it can be sized, and Snap works).
    pub sizebox: bool,
    /// `ptMinTrackSize` as the window procedure answers `WM_GETMINMAXINFO`.
    pub min_track: (i32, i32),
}

/// Sends a real `WM_NCHITTEST` for the screen point to the window and returns its answer.
pub fn send_hit_test(hwnd: isize, sx: i32, sy: i32) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
    let lp = ((sy as u16 as isize) << 16) | (sx as u16 as isize);
    // SAFETY: sends to a window of this process (from its own thread).
    unsafe { SendMessageW(hwnd as HWND, WM_NCHITTEST, 0, lp) as u32 }
}

/// Reads the window's rectangle, DPI, band, style and minimum track size.
pub fn report(hwnd: isize) -> EdgeReport {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongPtrW, WS_SIZEBOX};
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: reads and a message to a window of this process, into locals.
    unsafe {
        GetWindowRect(h, &mut r);
        let dpi = match GetDpiForWindow(h) {
            0 => 96,
            d => d,
        };
        let band = Band::at_dpi(dpi);
        EdgeReport {
            rect: [r.left, r.top, r.right, r.bottom],
            dpi,
            band: (band.x, band.y),
            sizebox: GetWindowLongPtrW(h, GWL_STYLE) as u32 & WS_SIZEBOX != 0,
            min_track: min_track(hwnd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SendMessageW, WS_POPUP, WS_SIZEBOX,
    };

    const W: i32 = 1000;
    const H: i32 = 640;

    fn band(pct: i32) -> Band {
        // The metrics Windows gives: 4 + 4 at 100 percent, scaled.
        let s = |v: i32| v * pct / 100;
        Band::from_metrics(s(4), s(4), s(4))
    }

    #[test]
    fn each_edge_answers_its_own_code() {
        let b = band(100);
        assert_eq!(hit_test(W, H, 2, H / 2, b), Some(HTLEFT));
        assert_eq!(hit_test(W, H, W - 2, H / 2, b), Some(HTRIGHT));
        assert_eq!(hit_test(W, H, W / 2, 2, b), Some(HTTOP));
        assert_eq!(hit_test(W, H, W / 2, H - 2, b), Some(HTBOTTOM));
    }

    #[test]
    fn each_corner_answers_its_own_code_and_reaches_further_than_an_edge() {
        let b = band(100);
        assert_eq!(hit_test(W, H, 1, 1, b), Some(HTTOPLEFT));
        assert_eq!(hit_test(W, H, W - 1, 1, b), Some(HTTOPRIGHT));
        assert_eq!(hit_test(W, H, 1, H - 1, b), Some(HTBOTTOMLEFT));
        assert_eq!(hit_test(W, H, W - 1, H - 1, b), Some(HTBOTTOMRIGHT));
        // Along the edge the corner reaches 2 x band (16), past the 8 px edge.
        assert_eq!(hit_test(W, H, 12, 2, b), Some(HTTOPLEFT));
        assert_eq!(hit_test(W, H, 2, 12, b), Some(HTTOPLEFT));
        assert_eq!(hit_test(W, H, 20, 2, b), Some(HTTOP));
        assert_eq!(hit_test(W, H, 2, 20, b), Some(HTLEFT));
        assert_eq!(hit_test(W, H, W - 12, H - 2, b), Some(HTBOTTOMRIGHT));
        assert_eq!(hit_test(W, H, W - 20, H - 2, b), Some(HTBOTTOM));
    }

    #[test]
    fn the_inside_is_the_client_and_outside_is_nothing() {
        let b = band(100);
        assert_eq!(hit_test(W, H, W / 2, H / 2, b), None);
        assert_eq!(hit_test(W, H, 8, 8, b), None);
        assert_eq!(hit_test(W, H, W - 9, H - 9, b), None);
        assert_eq!(hit_test(W, H, -1, 5, b), None);
        assert_eq!(hit_test(W, H, W, 5, b), None);
        assert_eq!(hit_test(W, H, 5, H, b), None);
    }

    #[test]
    fn the_band_scales_with_the_dpi_from_100_to_300_percent() {
        for pct in [100, 150, 225, 300] {
            let b = band(pct);
            assert_eq!(b.x, 8 * pct / 100, "4 + 4 px at 100 percent, scaled");
            let e = b.x;
            // The last pixel of the edge is the edge; the first inside is the client.
            assert_eq!(hit_test(W * 3, H * 3, e - 1, H, b), Some(HTLEFT), "{pct}%");
            assert_eq!(hit_test(W * 3, H * 3, e, H, b), None, "{pct}%");
            assert_eq!(
                hit_test(W * 3, H * 3, W * 3 - e, H, b),
                Some(HTRIGHT),
                "{pct}%"
            );
            assert_eq!(hit_test(W * 3, H * 3, W * 3 - e - 1, H, b), None, "{pct}%");
            // The corner reaches 2 x the edge.
            assert_eq!(
                hit_test(W * 3, H * 3, 2 * e - 1, 0, b),
                Some(HTTOPLEFT),
                "{pct}%"
            );
            assert_eq!(hit_test(W * 3, H * 3, 2 * e, 0, b), Some(HTTOP), "{pct}%");
        }
        // The real metrics are non-zero at every one of those DPIs and never shrink as it grows.
        let mut last = 0;
        for dpi in [96u32, 144, 216, 288] {
            let b = Band::at_dpi(dpi);
            assert!(b.x >= 1 && b.y >= 1 && b.x >= last, "{dpi} dpi: {b:?}");
            last = b.x;
        }
    }

    #[test]
    fn a_flat_window_has_no_band() {
        let r = |l, t, rr, b| RECT {
            left: l,
            top: t,
            right: rr,
            bottom: b,
        };
        let mon = r(0, 0, 1920, 1080);
        assert!(
            is_flat(true, false, r(-8, -8, 1928, 1088), mon),
            "maximized"
        );
        assert!(
            is_flat(false, true, r(100, 100, 900, 700), mon),
            "minimized"
        );
        assert!(is_flat(false, false, mon, mon), "full screen");
        assert!(!is_flat(false, false, r(100, 100, 900, 700), mon));
        // Snapped to a half is still a sized window.
        assert!(!is_flat(false, false, r(0, 0, 960, 1080), mon));
    }

    #[test]
    fn a_tiny_window_never_overlaps_its_corners() {
        let b = band(300);
        // Left and right edges meet in the middle of a window narrower than two bands.
        assert_eq!(hit_test(20, 100, 9, 50, b), Some(HTLEFT));
        assert_eq!(hit_test(20, 100, 10, 50, b), Some(HTRIGHT));
    }

    fn hidden_window() -> HWND {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DefWindowProcW, RegisterClassW, WNDCLASSW,
        };
        // A plain class whose procedure is `DefWindowProc` (the STATIC class answers HTTRANSPARENT to a
        // hit test, which would hide what winit's window does: answer HTCLIENT).
        let class: Vec<u16> = "FlareResizeTest\0".encode_utf16().collect();
        // SAFETY: registers a class of this process (a repeat registration fails harmlessly) and makes
        // a hidden, never shown popup window on this thread.
        unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_SIZEBOX,
                100,
                100,
                800,
                500,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        }
    }
    fn nchittest(h: HWND, x: i32, y: i32) -> u32 {
        let lp = ((y as u16 as isize) << 16) | (x as u16 as isize);
        // SAFETY: sends to a window of this thread.
        unsafe { SendMessageW(h, WM_NCHITTEST, 0, lp) as u32 }
    }

    /// What winit does: the whole window is client area, so there is no non-client border.
    unsafe extern "system" fn winit_like(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCCALCSIZE;
        if msg == WM_NCCALCSIZE && wparam != 0 {
            return 0;
        }
        // SAFETY: the next procedure in the chain.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    #[test]
    fn the_subclass_on_a_real_window_answers_ht_codes_at_the_edges_and_corners() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HTCLIENT, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        };
        let h = hidden_window();
        assert!(!h.is_null());
        // SAFETY: subclasses the window made above, then recalculates its frame, from its own thread.
        unsafe {
            SetWindowSubclass(h, Some(winit_like), 0x4E43_4353, 0);
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
        // Without the fix this window answers the client everywhere, as winit's does.
        assert_eq!(nchittest(h, 100, 350), HTCLIENT, "before install");
        assert_eq!(nchittest(h, 100, 100), HTCLIENT, "before install");
        assert!(install(h as isize));
        // Installing twice is one subclass.
        assert!(install(h as isize));
        // SAFETY: reads the DPI of the window made above.
        let dpi = unsafe { GetDpiForWindow(h) };
        let b = Band::at_dpi(if dpi == 0 { 96 } else { dpi });
        let (l, t, r, bo) = (100, 100, 900, 600);
        let mid_y = (t + bo) / 2;
        let mid_x = (l + r) / 2;
        assert_eq!(nchittest(h, l, mid_y), HTLEFT);
        assert_eq!(nchittest(h, r - 1, mid_y), HTRIGHT);
        assert_eq!(nchittest(h, mid_x, t), HTTOP);
        assert_eq!(nchittest(h, mid_x, bo - 1), HTBOTTOM);
        assert_eq!(nchittest(h, l, t), HTTOPLEFT);
        assert_eq!(nchittest(h, r - 1, t), HTTOPRIGHT);
        assert_eq!(nchittest(h, l, bo - 1), HTBOTTOMLEFT);
        assert_eq!(nchittest(h, r - 1, bo - 1), HTBOTTOMRIGHT);
        // One pixel inside the band is not ours: the window's own answer, whatever it is, is not an edge.
        let inside = nchittest(h, mid_x, mid_y);
        assert!(
            ![HTLEFT, HTRIGHT, HTTOP, HTBOTTOM].contains(&inside),
            "{inside}"
        );
        // The band is exactly b.x wide: its last pixel is the edge, the next is the client.
        assert_eq!(nchittest(h, l + b.x - 1, mid_y), HTLEFT);
        assert_eq!(nchittest(h, l + b.x, mid_y), HTCLIENT);
        assert_eq!(nchittest(h, r - b.x, mid_y), HTRIGHT);
        assert_eq!(nchittest(h, r - b.x - 1, mid_y), HTCLIENT);
        assert_eq!(nchittest(h, mid_x, t + b.y - 1), HTTOP);
        assert_eq!(nchittest(h, mid_x, t + b.y), HTCLIENT);
        // A corner reaches twice the band along the edge.
        assert_eq!(nchittest(h, l + 2 * b.x - 1, t), HTTOPLEFT);
        assert_eq!(nchittest(h, l + 2 * b.x, t), HTTOP);
        assert_eq!(nchittest(h, l, t + 2 * b.y - 1), HTTOPLEFT);
        assert_eq!(nchittest(h, l, t + 2 * b.y), HTLEFT);
        // SAFETY: destroys the window made above.
        unsafe { DestroyWindow(h) };
    }

    #[test]
    fn a_full_screen_position_is_recognised() {
        let mon = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(is_rect(-1920, 0, 1920, 1080, mon));
        assert!(!is_rect(-1920, 0, 1920, 1000, mon));
        assert!(!is_rect(-1900, 0, 1920, 1080, mon));
    }

    #[test]
    fn a_size_below_the_minimum_is_held_to_it() {
        assert_eq!(clamp_size(100, 100, (560, 400)), (560, 400));
        assert_eq!(clamp_size(1000, 100, (560, 400)), (1000, 400));
        assert_eq!(clamp_size(1000, 640, (560, 400)), (1000, 640));
    }

    unsafe extern "system" fn min_300_200(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, WM_GETMINMAXINFO};
        if msg == WM_GETMINMAXINFO {
            // SAFETY: for this message `lparam` points at a MINMAXINFO.
            unsafe { (*(lparam as *mut MINMAXINFO)).ptMinTrackSize = POINT { x: 300, y: 200 } };
            return 0;
        }
        // SAFETY: the next procedure in the chain.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    #[test]
    fn a_programmatic_resize_on_a_real_window_is_clamped() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_NOZORDER, SetWindowPos};
        let h = hidden_window();
        assert!(!h.is_null());
        // The window's own minimum (as winit answers it from Slint's min-width and min-height).
        // SAFETY: subclasses the window made above, from its own thread.
        unsafe { SetWindowSubclass(h, Some(min_300_200), 0x4D49_4E31, 0) };
        assert!(install(h as isize));
        let min = min_track(h as isize);
        assert_eq!(min, (300, 200));
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: sizes and reads a hidden window of this thread.
        unsafe {
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                0,
                0,
                10,
                10,
                SWP_NOMOVE | SWP_NOZORDER,
            );
            GetWindowRect(h, &mut r);
            DestroyWindow(h);
        }
        assert_eq!((r.right - r.left, r.bottom - r.top), min);
    }
}
