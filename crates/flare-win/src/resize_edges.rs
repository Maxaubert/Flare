//! Native resize edges and corners on the frameless main window.
//!
//! Slint's `no-frame` window is a winit undecorated window: winit keeps `WS_SIZEBOX` on it (so Aero
//! Snap and the snap layouts work) but answers `WM_NCCALCSIZE` with the whole window as client area,
//! so there is no non-client border and `DefWindowProc` answers `WM_NCHITTEST` with `HTCLIENT`
//! everywhere: no handles, no resize cursors. Prism Terminal (Electron frameless) resizes from
//! invisible edges; this subclass gives Flare the same behaviour and sizes without any visible frame. It answers
//! `WM_NCHITTEST` with `HTLEFT`..`HTBOTTOMRIGHT` inside a band along the window's edge and passes
//! everything else on, so the title-bar drag, the window buttons, the tab strip and the terminal are
//! untouched. `DefWindowProc` then supplies the resize cursors and the sizing loop, and
//! `WM_GETMINMAXINFO` (winit, from Slint's `min-width`/`min-height`, 560 x 400 as Prism Terminal)
//! keeps the minimum.
//!
//! The sizes are Prism Terminal's Electron frameless zones: an edge band of `round(5 * dpi / 96)`
//! device px and a corner `round(16 * dpi / 96)` long, both inside the window, at the window's own
//! DPI read on every hit test, so a `WM_DPICHANGED` and a monitor move need no work. No band while
//! maximized, minimized or full screen.
//!
//! It also answers `WM_NCCALCSIZE` for the frameless window itself (`frameless_client`): Slint
//! turns winit's decorations on while the window is full screen and off again one update after it
//! leaves, and in between `DefWindowProc` would make room for a caption (the UI jumped on leaving
//! full screen).

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongW, GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT,
    HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsIconic, IsZoomed, SWP_NOSIZE, WINDOWPOS,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_WINDOWPOSCHANGING, WS_CAPTION,
};

/// The subclass id (one per window).
const RESIZE_ID: usize = 0x464C_5253;

/// The thickness of the resize band and the reach of a corner, in device px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Left, right and bottom edges.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// How far a corner reaches along each edge.
    pub corner: i32,
}

/// Prism Terminal's (Electron frameless) resize zones, in css px at 96 dpi.
const EDGE_PX: f64 = 5.0;
const CORNER_PX: f64 = 16.0;

impl Band {
    /// The band at a DPI: `round(5 * dpi / 96)` for an edge, `round(16 * dpi / 96)` for a corner
    /// (at least 1 px), as Prism Terminal's window has them.
    pub fn for_dpi(dpi: u32) -> Self {
        let s = f64::from(dpi.max(1)) / 96.0;
        let edge = ((EDGE_PX * s).round() as i32).max(1);
        Self {
            x: edge,
            y: edge,
            corner: ((CORNER_PX * s).round() as i32).max(edge),
        }
    }

    /// The band at a DPI.
    pub fn at_dpi(dpi: u32) -> Self {
        Self::for_dpi(dpi)
    }
}

/// Which handle a point of the window is, as the `HT*` code `WM_NCHITTEST` answers.
/// `x` and `y` are relative to the window's top-left, `w` and `h` its size, all in device px.
/// `None` is the rest of the window (the client).
pub fn hit_test(w: i32, h: i32, x: i32, y: i32, band: Band) -> Option<u32> {
    if x < 0 || y < 0 || x >= w || y >= h {
        return None;
    }
    // A corner reaches its own length along each edge, but never past the middle.
    let (bx, by) = (band.x.min(w / 2), band.y.min(h / 2));
    let cx = band.corner.min(w / 2);
    let cy = band.corner.min(h / 2);
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
    if msg == WM_NCCALCSIZE {
        // The window is frameless (`no-frame`), but Slint asks winit for decorations while it is full
        // screen and takes them off again only on its next property update: leaving full screen,
        // the caption style comes back while winit still has decorations on, its `WM_NCCALCSIZE`
        // falls to `DefWindowProc`, and for about 30 ms the client leaves room for a caption the
        // window never draws (the whole UI jumps down and back). Answered here as winit answers a
        // frameless window: a maximized one gets the work area, any other the whole window.
        // SAFETY: for this message `lparam` points at a RECT (`wparam` 0) or at NCCALCSIZE_PARAMS,
        // whose first member is the proposed window RECT; either way it becomes the client RECT.
        let r = unsafe { &mut *(lparam as *mut RECT) };
        if let Some(client) = frameless_client(hwnd, r) {
            *r = client;
            return 0;
        }
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

/// The client RECT of the frameless window at the proposed window RECT, as winit gives it: the
/// work area of the monitor under it when maximized, else the whole RECT. `None` (left to the
/// chain) without the caption style, which is full screen, and when minimized.
fn frameless_client(hwnd: HWND, proposed: &RECT) -> Option<RECT> {
    // SAFETY: reads the state and monitor of a window of this process into locals.
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & WS_CAPTION != WS_CAPTION || IsIconic(hwnd) != 0 {
            return None;
        }
        if IsZoomed(hwnd) == 0 {
            return Some(*proposed);
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mon = MonitorFromRect(proposed, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        Some(mi.rcWork)
    }
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
    /// How far a corner reaches along each edge, in device px.
    pub corner: i32,
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
            corner: band.corner,
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
        Band::for_dpi((96 * pct / 100) as u32)
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
        // Along the edge the corner reaches 16 px (Prism Terminal's), past the 5 px edge.
        assert_eq!(hit_test(W, H, 15, 2, b), Some(HTTOPLEFT));
        assert_eq!(hit_test(W, H, 2, 15, b), Some(HTTOPLEFT));
        assert_eq!(hit_test(W, H, 16, 2, b), Some(HTTOP));
        assert_eq!(hit_test(W, H, 2, 16, b), Some(HTLEFT));
        assert_eq!(hit_test(W, H, W - 16, H - 2, b), Some(HTBOTTOMRIGHT));
        assert_eq!(hit_test(W, H, W - 17, H - 2, b), Some(HTBOTTOM));
    }

    #[test]
    fn the_inside_is_the_client_and_outside_is_nothing() {
        let b = band(100);
        assert_eq!(hit_test(W, H, W / 2, H / 2, b), None);
        assert_eq!(hit_test(W, H, 5, 5, b), None);
        assert_eq!(hit_test(W, H, W - 6, H - 6, b), None);
        // The Close button's right edge is the client at 100 percent (it was a resize edge at 8 px).
        assert_eq!(hit_test(W, H, W - 6, 20, b), None);
        // The Close button's own last pixel is w - 7 (TitleButtons pads 6 px on the right): the
        // client at 100 percent, where an 8 px band made it a resize edge; at 300 percent the
        // padding is 18 px and the band 15.
        assert_eq!(hit_test(W, H, W - 7, 18, b), None);
        assert_eq!(
            hit_test(
                W,
                H,
                W - 7,
                18,
                Band {
                    x: 8,
                    y: 8,
                    corner: 16
                }
            ),
            Some(HTRIGHT)
        );
        assert_eq!(hit_test(W, H, W - 19, 54, band(300)), None);
        assert_eq!(hit_test(W, H, -1, 5, b), None);
        assert_eq!(hit_test(W, H, W, 5, b), None);
        assert_eq!(hit_test(W, H, 5, H, b), None);
    }

    #[test]
    fn the_band_scales_with_the_dpi_from_100_to_300_percent() {
        for pct in [100, 150, 225, 300] {
            let b = band(pct);
            // Literal sizes (5 and 16 px at 96 dpi, scaled and rounded), so a floor or a ceil in
            // the code shows at once: 7.5 -> 8, 11.25 -> 11, 24 and 36 and 48 exactly.
            let (want_edge, want_corner) = match pct {
                100 => (5, 16),
                150 => (8, 24),
                225 => (11, 36),
                _ => (15, 48),
            };
            assert_eq!(
                (b.x, b.y, b.corner),
                (want_edge, want_edge, want_corner),
                "{pct}%"
            );
            let (e, c) = (b.x, b.corner);
            // The last pixel of the edge is the edge; the first inside is the client.
            assert_eq!(hit_test(W * 3, H * 3, e - 1, H, b), Some(HTLEFT), "{pct}%");
            assert_eq!(hit_test(W * 3, H * 3, e, H, b), None, "{pct}%");
            assert_eq!(
                hit_test(W * 3, H * 3, W * 3 - e, H, b),
                Some(HTRIGHT),
                "{pct}%"
            );
            assert_eq!(hit_test(W * 3, H * 3, W * 3 - e - 1, H, b), None, "{pct}%");
            // The corner reaches its own length.
            assert_eq!(
                hit_test(W * 3, H * 3, c - 1, 0, b),
                Some(HTTOPLEFT),
                "{pct}%"
            );
            assert_eq!(hit_test(W * 3, H * 3, c, 0, b), Some(HTTOP), "{pct}%");
        }
        // The band is non-zero at every one of those DPIs and never shrinks as it grows.
        let mut last = 0;
        for dpi in [96u32, 144, 216, 288] {
            let b = Band::for_dpi(dpi);
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
    fn the_window_leaving_full_screen_never_leaves_room_for_a_caption() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            NCCALCSIZE_PARAMS, SendMessageW, SetWindowLongW, WS_MAXIMIZE, WS_OVERLAPPEDWINDOW,
            WS_POPUP,
        };
        let h = hidden_window();
        assert!(!h.is_null());
        // Both forms of the message: a bare RECT (`wparam` 0) and NCCALCSIZE_PARAMS (`wparam` 1, the
        // one a frame change sends). Once the subclass is in, they agree.
        let installed = std::cell::Cell::new(false);
        let calc = |r: RECT| -> RECT {
            let mut bare = r;
            // SAFETY: zeroed params with the proposed RECT first, then two messages with locals, to a
            // window of this thread.
            let full = unsafe {
                let mut p: NCCALCSIZE_PARAMS = std::mem::zeroed();
                p.rgrc[0] = r;
                SendMessageW(
                    h,
                    WM_NCCALCSIZE,
                    1,
                    &mut p as *mut NCCALCSIZE_PARAMS as LPARAM,
                );
                SendMessageW(h, WM_NCCALCSIZE, 0, &mut bare as *mut RECT as LPARAM);
                p.rgrc[0]
            };
            if installed.get() {
                assert_eq!(
                    (bare.left, bare.top, bare.right, bare.bottom),
                    (full.left, full.top, full.right, full.bottom)
                );
            }
            full
        };
        let tuple = |r: RECT| (r.left, r.top, r.right, r.bottom);
        // SAFETY: reads the monitor of the window made above into locals.
        let (monitor, work) = unsafe {
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi);
            (mi.rcMonitor, mi.rcWork)
        };
        let set_style = |style: u32| {
            // SAFETY: changes the style of the hidden window made above (no frame change is asked).
            unsafe { SetWindowLongW(h, GWL_STYLE, style as i32) };
        };
        let sized = RECT {
            left: 100,
            top: 100,
            right: 900,
            bottom: 600,
        };
        // The caption style is back, as just after full screen while winit still has decorations
        // on: without the subclass `DefWindowProc` leaves room for a caption.
        set_style(WS_OVERLAPPEDWINDOW | WS_MAXIMIZE);
        let mut bare = monitor;
        // SAFETY: the `wparam` 0 form with a local RECT, to a window of this thread.
        unsafe { SendMessageW(h, WM_NCCALCSIZE, 0, &mut bare as *mut RECT as LPARAM) };
        assert!(bare.top > work.top, "{}", bare.top);
        assert!(install(h as isize));
        installed.set(true);
        // Maximized: the work area, as winit answers its frameless window.
        assert_eq!(tuple(calc(monitor)), tuple(work));
        // A sized window: the whole window.
        set_style(WS_OVERLAPPEDWINDOW);
        assert_eq!(tuple(calc(sized)), tuple(sized));
        // Full screen (no caption style) is left to the chain: the whole monitor.
        set_style(WS_POPUP | WS_MAXIMIZE);
        assert_eq!(tuple(calc(monitor)), tuple(monitor));
        // SAFETY: destroys the window made above.
        unsafe { DestroyWindow(h) };
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
        let b = Band::for_dpi(if dpi == 0 { 96 } else { dpi });
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
        // A corner reaches its own length along the edge.
        assert_eq!(nchittest(h, l + b.corner - 1, t), HTTOPLEFT);
        assert_eq!(nchittest(h, l + b.corner, t), HTTOP);
        assert_eq!(nchittest(h, l, t + b.corner - 1), HTTOPLEFT);
        assert_eq!(nchittest(h, l, t + b.corner), HTLEFT);
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
