//! A look at a window of the app from OUTSIDE, with no pipe: the visual tour's start-up capture
//! (audit 2026-10-20) needs the frames from process creation, before the test hook's pipe exists.
//!
//! `windows_of` lists a process's top-level windows (largest first) and `capture` copies one with
//! `PrintWindow` and `PW_RENDERFULLCONTENT` into a 32-bit DIB, the way the app's own fallback snapshot
//! does. Neither activates, moves, shows or types into a window: `PrintWindow` only asks the window to
//! paint itself. Only ever pointed at a window of a process this run started.

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId,
};

#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
    fn SetThreadDpiAwarenessContext(context: isize) -> isize;
    fn IsWindowVisible(hwnd: HWND) -> i32;
    fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> i32;
    fn GetWindowLongW(hwnd: HWND, index: i32) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(
        hwnd: HWND,
        attribute: u32,
        value: *mut core::ffi::c_void,
        size: u32,
    ) -> i32;
}

/// What the screen can show of a window at one moment, read from the window manager rather than from a
/// copy: `PrintWindow` asks the window to paint itself, so a copy of a window that is not on the screen
/// (not yet shown, or cloaked) shows what DWM never composes (audit V1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinState {
    /// `WS_VISIBLE` is set and DWM has not cloaked the window: it is composed on the screen.
    pub on_screen: bool,
    /// The window rectangle, device pixels.
    pub w: i32,
    pub h: i32,
    /// The client rectangle, device pixels. A frameless window's client is the whole window; a smaller
    /// client means DWM draws a frame (caption and border) around it.
    pub cw: i32,
    pub ch: i32,
    /// `GWL_STYLE`.
    pub style: u32,
    /// `GWL_EXSTYLE`.
    pub ex_style: u32,
}

impl WinState {
    /// The client is smaller than the window: a frame is drawn around it.
    pub fn framed(&self) -> bool {
        self.cw < self.w || self.ch < self.h
    }
}

/// The window's state now, or None when it is gone.
pub fn state(hwnd: isize) -> Option<WinState> {
    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const DWMWA_CLOAKED: u32 = 14;
    let _device = DeviceCoordinates::enter();
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut c = r;
    // SAFETY: a window handle and valid out pointers; a gone window just fails.
    unsafe {
        if GetWindowRect(h, &mut r) == 0 || GetClientRect(h, &mut c) == 0 {
            return None;
        }
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let mut cloaked = 0u32;
        let got = DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        let visible = IsWindowVisible(h) != 0 && style & WS_VISIBLE != 0;
        Some(WinState {
            on_screen: visible && (got != 0 || cloaked == 0),
            w: r.right - r.left,
            h: r.bottom - r.top,
            cw: c.right - c.left,
            ch: c.bottom - c.top,
            style,
            ex_style,
        })
    }
}

/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`.
const PER_MONITOR_AWARE_V2: isize = -4;

/// This thread sees device pixels until the guard drops. The app's windows are per-monitor aware; asked
/// from a DPI-unaware thread, `GetWindowRect` answers a scaled-down size and `PrintWindow` copies a
/// crop of the window with a black block where the smaller bitmap did not reach (audit V2).
struct DeviceCoordinates(isize);

impl DeviceCoordinates {
    fn enter() -> Self {
        // SAFETY: changes only this thread's DPI awareness, restored on drop.
        Self(unsafe { SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DeviceCoordinates {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: puts back the context this thread had.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

/// One captured window, straight RGBA8, device pixels.
#[derive(Clone, Debug)]
pub struct Shot {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// A window and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinInfo {
    pub hwnd: isize,
    pub w: i32,
    pub h: i32,
}

struct Collect {
    pid: u32,
    found: Vec<WinInfo>,
}

unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: `lparam` is the address of the `Collect` that `windows_of` keeps alive for the call.
    let c = unsafe { &mut *(lparam as *mut Collect) };
    let mut pid = 0u32;
    // SAFETY: a window handle from the enumeration and a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == c.pid {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: as above.
        if unsafe { GetWindowRect(hwnd, &mut r) } != 0 {
            c.found.push(WinInfo {
                hwnd: hwnd as isize,
                w: r.right - r.left,
                h: r.bottom - r.top,
            });
        }
    }
    1
}

/// The top-level windows of process `pid` with an area, the largest first.
pub fn windows_of(pid: u32) -> Vec<WinInfo> {
    let _device = DeviceCoordinates::enter();
    let mut c = Collect {
        pid,
        found: Vec::new(),
    };
    // SAFETY: the callback only reads window data and writes into `c`, which outlives the call.
    unsafe {
        EnumWindows(Some(each), &mut c as *mut Collect as LPARAM);
    }
    let mut found: Vec<WinInfo> = c
        .found
        .into_iter()
        .filter(|w| w.w > 50 && w.h > 50)
        .collect();
    found.sort_by_key(|w| std::cmp::Reverse(i64::from(w.w) * i64::from(w.h)));
    found
}

/// How a window is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `PrintWindow` with `PW_RENDERFULLCONTENT`: right for a GPU swapchain.
    PrintWindow,
    /// `PrintWindow` with `PW_CLIENTONLY` and without `PW_RENDERFULLCONTENT`: the window paints its own
    /// GDI surface, which is what the software renderer presents. The window keeps a classic caption
    /// style that a copy with the frame would draw over it. With the full-content flag that window
    /// comes back with a black block where the client area is, though its surface is fully painted
    /// (audit V2).
    Gdi,
}

/// Copies the window `hwnd` as it paints itself now (`PrintWindow`).
pub fn capture(hwnd: isize) -> Result<Shot, String> {
    capture_with(hwnd, Method::PrintWindow)
}

/// Copies the window `hwnd` as it paints itself now, the way `method` says.
pub fn capture_with(hwnd: isize, method: Method) -> Result<Shot, String> {
    const PW_CLIENTONLY: u32 = 1;
    const PW_RENDERFULLCONTENT: u32 = 2;
    let _device = DeviceCoordinates::enter();
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: a window handle and a valid out pointer; a gone window just fails.
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        return Err("no window rectangle".into());
    }
    let (w, hh) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || hh <= 0 {
        return Err("the window is empty".into());
    }
    // SAFETY: plain GDI calls; every object made here is released before returning, and the DIB's
    // memory is only read while the bitmap is alive.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return Err("no device context".into());
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -hh,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return Err("no bitmap".into());
        }
        let old = SelectObject(dc, bmp);
        let ok = match method {
            Method::PrintWindow => PrintWindow(h, dc, PW_RENDERFULLCONTENT),
            Method::Gdi => PrintWindow(h, dc, PW_CLIENTONLY),
        };
        let n = w as usize * hh as usize * 4;
        let src = std::slice::from_raw_parts(bits as *const u8, n);
        let mut rgba = Vec::with_capacity(n);
        for p in src.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        if ok == 0 {
            return Err("PrintWindow failed".into());
        }
        Ok(Shot {
            w: w as u32,
            h: hh as u32,
            rgba,
        })
    }
}

/// What the tour records about one frame without keeping its pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub w: u32,
    pub h: u32,
    /// FNV-1a over every pixel: two frames with the same hash and size are the same picture.
    pub hash: u64,
    /// The mean colour of a sparse sample.
    pub mean: [f64; 3],
    /// The colour of the pixel in the middle of the first row under the top edge (the bar).
    pub bar: [u8; 3],
    /// The colour of the pixel in the middle of the bottom edge (the terminal's padding).
    pub foot: [u8; 3],
    /// True when the whole picture is one colour (a blank frame).
    pub blank: bool,
}

/// The cheap facts of a frame.
pub fn look(s: &Shot) -> Look {
    let px = |x: u32, y: u32| -> [u8; 3] {
        let i = ((y.min(s.h.saturating_sub(1)) * s.w + x.min(s.w.saturating_sub(1))) * 4) as usize;
        if i + 2 < s.rgba.len() {
            [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2]]
        } else {
            [0, 0, 0]
        }
    };
    let (pixels, _) = s.rgba.as_chunks::<4>();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    let mut first: Option<[u8; 3]> = None;
    let mut blank = true;
    for (i, p) in pixels.iter().enumerate() {
        hash ^= u64::from(u32::from_le_bytes([p[0], p[1], p[2], 0]));
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        let c = [p[0], p[1], p[2]];
        match first {
            None => first = Some(c),
            Some(f) if f != c => blank = false,
            Some(_) => {}
        }
        if i % 7 == 0 {
            sum[0] += f64::from(p[0]);
            sum[1] += f64::from(p[1]);
            sum[2] += f64::from(p[2]);
            n += 1.0;
        }
    }
    let n = n.max(1.0);
    Look {
        w: s.w,
        h: s.h,
        hash,
        mean: [sum[0] / n, sum[1] / n, sum[2] / n],
        bar: px(s.w / 2, 3),
        foot: px(s.w / 2, s.h.saturating_sub(4)),
        blank,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(w: u32, h: u32, c: [u8; 3]) -> Shot {
        let mut rgba = Vec::new();
        for _ in 0..w * h {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        Shot { w, h, rgba }
    }

    #[test]
    fn one_colour_is_blank_and_two_are_not() {
        let mut s = shot(40, 30, [20, 30, 40]);
        let l = look(&s);
        assert!(l.blank);
        assert_eq!(l.bar, [20, 30, 40]);
        assert_eq!(l.foot, [20, 30, 40]);
        // Paint every pixel of the first row so the sparse sample meets it.
        for x in 0..40usize {
            s.rgba[x * 4] = 250;
        }
        let m = look(&s);
        assert!(!m.blank);
        assert_ne!(l.hash, m.hash);
    }

    #[test]
    fn the_same_picture_has_the_same_hash_and_another_size_does_not_match() {
        let a = look(&shot(40, 30, [1, 2, 3]));
        let b = look(&shot(40, 30, [1, 2, 3]));
        assert_eq!(a, b);
        let c = look(&shot(80, 30, [1, 2, 3]));
        assert_ne!((a.w, a.h), (c.w, c.h));
    }

    #[test]
    fn a_process_with_no_window_lists_none() {
        // Pid 0 never owns a top-level window.
        assert!(windows_of(0).is_empty());
    }
}
