//! The double-click time and distance from the system.

#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
#[cfg(windows)]
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
#[cfg(windows)]
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

/// macOS: the user's double-click speed (`NSEvent.doubleClickInterval`, System Settings, Mouse).
/// The Mac has no drift setting; the fallback's distance is kept.
#[cfg(target_os = "macos")]
pub fn system_double_click() -> DoubleClick {
    mac_double_click(objc2_app_kit::NSEvent::doubleClickInterval())
}

/// Pure: the interval in seconds as the metrics, the fallback for an answer that makes no sense.
#[cfg(any(target_os = "macos", test))]
fn mac_double_click(secs: f64) -> DoubleClick {
    if !secs.is_finite() || secs <= 0.0 || secs > 5.0 {
        return FALLBACK;
    }
    DoubleClick {
        time_ms: (secs * 1000.0).round() as u64,
        ..FALLBACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_interval_is_in_ms_and_a_strange_one_falls_back() {
        assert_eq!(mac_double_click(0.5).time_ms, 500);
        assert_eq!(mac_double_click(0.25).time_ms, 250);
        for bad in [0.0, -1.0, f64::NAN, 60.0] {
            assert_eq!(mac_double_click(bad), FALLBACK, "{bad}");
        }
    }

    #[test]
    fn the_double_click_metrics_are_sane() {
        let d = system_double_click();
        assert!((100..=5000).contains(&d.time_ms), "{d:?}");
        assert!(d.dx >= 2.0 && d.dy >= 2.0, "{d:?}");
    }
}
