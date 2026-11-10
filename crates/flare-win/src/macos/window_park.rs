//! Parking the --e2e window off Windows. macOS: the Mac E2E runs in CI only (the port plan's
//! "Ongoing" row, accessory policy); until then nothing is parked and the report says so.

/// Where a parked window would sit: far outside every monitor's rectangle.
const PARK_AT: i32 = -32000;

/// The top-left corner of a parked window, in device pixels.
pub fn parked_origin() -> (i32, i32) {
    (PARK_AT, PARK_AT)
}

/// Not built yet: the window is left where it is.
pub fn park(_hwnd: isize) {}

/// What the harness checks of an `--e2e` window (the same shape as on Windows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParkReport {
    pub rect: (i32, i32, i32, i32),
    pub ex_style: u32,
    pub screen: (i32, i32, i32, i32),
    pub foreground: bool,
}

impl ParkReport {
    /// Unknown here: false.
    pub fn on_screen(&self) -> bool {
        false
    }

    /// Always one problem until the Mac E2E parks windows.
    pub fn problems(&self) -> Vec<String> {
        vec!["the window cannot be parked on this platform yet".into()]
    }
}

/// An empty report.
pub fn report(_hwnd: isize) -> ParkReport {
    ParkReport::default()
}
