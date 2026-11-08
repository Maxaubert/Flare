//! work_areas() with scales (Task 14).
//!
//! The monitors as Windows has them now: each work area in device pixels (the window's usable rectangle,
//! without the taskbar), and the scale a monitor shows at, so a window that moves between monitors of
//! different scales keeps its size in logical pixels.

#[cfg(windows)]
use std::mem::size_of;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT, TRUE};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
#[cfg(windows)]
use windows_sys::core::BOOL;

use crate::window_state::Area;

/// Every monitor's work area, in device pixels. Empty when the enumeration fails.
#[cfg(windows)]
pub fn work_areas() -> Vec<Area> {
    let mut areas: Vec<Area> = Vec::new();
    // SAFETY: EnumDisplayMonitors calls `monitor_proc` synchronously on this thread before it returns,
    // and `areas` outlives that call; the callback is the only thing that touches it through the pointer.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(monitor_proc),
            &mut areas as *mut Vec<Area> as LPARAM,
        );
    }
    areas
}

/// Called once per monitor by EnumDisplayMonitors; `data` is the `&mut Vec<Area>` from `work_areas`.
#[cfg(windows)]
unsafe extern "system" fn monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rcWork: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dwFlags: 0,
    };
    // SAFETY: `info` is a MONITORINFO with cbSize set, as GetMonitorInfoW requires, and `hmon` is the
    // handle the enumeration just gave us.
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info) };
    if ok != 0 {
        // SAFETY: `data` is the pointer `work_areas` passed in: alive until EnumDisplayMonitors returns,
        // and no other reference to the Vec exists while the callback runs.
        let areas = unsafe { &mut *(data as *mut Vec<Area>) };
        let r = info.rcWork;
        areas.push(Area {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        });
    }
    TRUE
}

/// The effective DPI of the monitor holding the point, divided by 96. 1.0 on any failure.
#[cfg(windows)]
pub fn scale_at(x: i32, y: i32) -> f64 {
    let pt = POINT { x, y };
    // SAFETY: MonitorFromPoint takes the point by value and returns a monitor handle or null.
    let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if hmon.is_null() {
        return 1.0;
    }
    let mut dpi_x: u32 = 0;
    let mut dpi_y: u32 = 0;
    // SAFETY: `hmon` is a handle just returned by MonitorFromPoint; both out-pointers are valid u32s.
    let hr = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    if hr == 0 && dpi_x > 0 {
        f64::from(dpi_x) / 96.0
    } else {
        1.0
    }
}

/// macOS: phase 3 of the port plan (NSScreen visibleFrame); no monitor is known until then.
#[cfg(not(windows))]
pub fn work_areas() -> Vec<Area> {
    Vec::new()
}

/// macOS: phase 3 of the port plan (NSScreen backingScaleFactor); 1.0 until then.
#[cfg(not(windows))]
pub fn scale_at(_x: i32, _y: i32) -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_areas_have_positive_sizes() {
        for a in work_areas() {
            assert!(a.width > 0 && a.height > 0, "{a:?}");
        }
        let s = scale_at(0, 0);
        assert!(s >= 1.0, "scale {s}");
    }
}
