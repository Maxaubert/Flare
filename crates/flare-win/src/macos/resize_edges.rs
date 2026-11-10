//! Resize edges off Windows. macOS: phase 3 of the port plan; a native window keeps its own edges,
//! so there is nothing to install and the e2e probe reads an empty report.

/// What the window says about its resize edges (the same shape as on Windows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeReport {
    pub rect: [i32; 4],
    pub dpi: u32,
    pub band: (i32, i32),
    pub corner: i32,
    pub sizebox: bool,
    pub min_track: (i32, i32),
}

/// Nothing to install: false.
pub fn install(_hwnd: isize) -> bool {
    false
}

/// No window procedure to ask: 0 (HTNOWHERE).
pub fn send_hit_test(_hwnd: isize, _sx: i32, _sy: i32) -> u32 {
    0
}

/// An empty report.
pub fn report(_hwnd: isize) -> EdgeReport {
    EdgeReport::default()
}
