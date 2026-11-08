//! Resize edges on macOS (port phase 3): not applicable, on purpose. On Windows the frameless
//! window has no non-client border, so Flare answers WM_NCHITTEST itself. The Mac window is a
//! titled, resizable `NSWindow` (a transparent full-size titlebar, `flare::backend`), so AppKit
//! keeps its own resize edges and corners, its own cursors, the double-click-an-edge fill and the
//! minimum size winit hands it. Installing nothing here is what lets AppKit handle all of it; the
//! e2e probe reads an empty report.

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

/// Nothing to install (AppKit owns the edges): false.
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
