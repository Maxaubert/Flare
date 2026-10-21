//! The double-click time and distance from the system.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
};

/// The system's double-click time and the distance a second click may drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoubleClick {
    pub time_ms: u64,
    pub dx: f64,
    pub dy: f64,
}

/// Used when the system cannot answer (never in practice).
pub const FALLBACK: DoubleClick = DoubleClick {
    time_ms: 500,
    dx: 4.0,
    dy: 4.0,
};

/// The system's double-click time and distance.
pub fn system_double_click() -> DoubleClick {
    // SAFETY: GetDoubleClickTime takes no arguments and only reads a system setting.
    let time = unsafe { GetDoubleClickTime() };
    // SAFETY: GetSystemMetrics takes a metric id and only reads a system setting.
    let dx = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) };
    // SAFETY: GetSystemMetrics takes a metric id and only reads a system setting.
    let dy = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) };
    if time == 0 || dx <= 0 || dy <= 0 {
        return FALLBACK;
    }
    DoubleClick {
        time_ms: u64::from(time),
        dx: f64::from(dx),
        dy: f64::from(dy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_double_click_metrics_are_sane() {
        let d = system_double_click();
        assert!((100..=5000).contains(&d.time_ms), "{d:?}");
        assert!(d.dx >= 2.0 && d.dy >= 2.0, "{d:?}");
    }
}
