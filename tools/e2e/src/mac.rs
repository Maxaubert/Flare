//! What the harness reads of a Mac desktop (rule 1104): the window server's on-screen list, the
//! frontmost app and the pasteboard's change count. None of these needs Screen Recording or
//! Accessibility rights, so a CI runner can read them. Pixels never come from here: a scenario's
//! pictures are the app's own render (`Snapshot`, Slint's snapshot of the window).

use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSWorkspace};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGDisplayBounds, CGError, CGGetActiveDisplayList, CGWindowListCopyWindowInfo,
    CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// One window the window server lists as ordered in: its owner, its bounds (global display points,
/// top-left origin) and its alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub pid: u32,
    pub bounds: (f64, f64, f64, f64),
    pub alpha: f64,
    /// The window level (0 normal, 3 floating, 101 a menu, 25 the status bar ...).
    pub layer: i64,
    /// The owner's name (needs no right, unlike the window's own title).
    pub owner: String,
    /// The window server's number for the window (`kCGWindowNumber`).
    pub number: i64,
}

/// Every window the window server lists as ordered in (`OnScreenOnly`: ordered in, wherever it is).
pub fn listed_windows() -> Vec<Listed> {
    let Some(arr) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0) else {
        return Vec::new();
    };
    let ptr = CFRetained::as_ptr(&arr).as_ptr();
    // SAFETY: CFArray and NSArray are toll-free bridged; `arr` stays alive for the whole borrow.
    let list: &NSArray = unsafe { &*(ptr.cast::<NSArray>()) };
    let key = NSString::from_str;
    let num = |d: &NSDictionary, k: &str| -> Option<f64> {
        d.objectForKey(&key(k))
            .and_then(|p| p.downcast::<NSNumber>().ok())
            .map(|n| n.doubleValue())
    };
    list.iter()
        .filter_map(|d| {
            let d: Retained<NSDictionary> = d.downcast().ok()?;
            let pid = u32::try_from(num(&d, "kCGWindowOwnerPID")? as i64).ok()?;
            let b: Retained<NSDictionary> = d
                .objectForKey(&key("kCGWindowBounds"))
                .and_then(|b| b.downcast().ok())?;
            let bounds = (
                num(&b, "X")?,
                num(&b, "Y")?,
                num(&b, "Width")?,
                num(&b, "Height")?,
            );
            let alpha = num(&d, "kCGWindowAlpha").unwrap_or(1.0);
            let layer = num(&d, "kCGWindowLayer").unwrap_or(0.0) as i64;
            let owner = d
                .objectForKey(&key("kCGWindowOwnerName"))
                .and_then(|p| p.downcast::<NSString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let number = num(&d, "kCGWindowNumber").unwrap_or(0.0) as i64;
            Some(Listed {
                pid,
                bounds,
                alpha,
                layer,
                owner,
                number,
            })
        })
        .collect()
}

/// The bounds of every active display (global points, top-left origin).
pub fn display_bounds() -> Vec<(f64, f64, f64, f64)> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: the buffer holds 16 ids and the count is a valid out pointer.
    let err = unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) };
    if err != CGError::Success {
        return Vec::new();
    }
    ids[..(n as usize).min(16)]
        .iter()
        .map(|id| {
            let r = CGDisplayBounds(*id);
            (r.origin.x, r.origin.y, r.size.width, r.size.height)
        })
        .collect()
}

/// Pure: whether a window with `bounds` and `alpha` shows any pixel on one of `displays`.
pub fn shows_on(
    bounds: (f64, f64, f64, f64),
    alpha: f64,
    displays: &[(f64, f64, f64, f64)],
) -> bool {
    let (x, y, w, h) = bounds;
    alpha > 0.0
        && w > 0.0
        && h > 0.0
        && displays
            .iter()
            .any(|&(dx, dy, dw, dh)| x < dx + dw && dx < x + w && y < dy + dh && dy < y + h)
}

/// The windows of the processes `pids` that show on a display (ordered in, not transparent, and
/// overlapping a display: a parked window at (-32000, -32000) is ordered in but shows nowhere).
pub fn shown_windows_of(pids: &[u32]) -> Vec<Listed> {
    let displays = display_bounds();
    listed_windows()
        .into_iter()
        .filter(|w| pids.contains(&w.pid) && shows_on(w.bounds, w.alpha, &displays))
        .collect()
}

/// How many windows of the processes `pids` show on a display.
pub fn on_screen_windows_of(pids: &[u32]) -> usize {
    shown_windows_of(pids).len()
}

/// The pid of the frontmost app (the one that has the keyboard), None when the workspace cannot say.
pub fn frontmost_pid() -> Option<u32> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    u32::try_from(app.processIdentifier())
        .ok()
        .filter(|p| *p != 0)
}

/// The general pasteboard's change count: it moves on every write by any app.
pub fn pasteboard_change_count() -> Option<u32> {
    let n = NSPasteboard::generalPasteboard().changeCount();
    u32::try_from(n).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_or_clear_window_shows_nowhere() {
        let screen = [(0.0, 0.0, 1512.0, 982.0)];
        assert!(shows_on((100.0, 100.0, 800.0, 600.0), 1.0, &screen));
        assert!(!shows_on((-32000.0, -32000.0, 1000.0, 640.0), 1.0, &screen));
        assert!(!shows_on((100.0, 100.0, 800.0, 600.0), 0.0, &screen));
        assert!(
            shows_on((-500.0, 10.0, 501.0, 5.0), 1.0, &screen),
            "one column on screen is on screen"
        );
    }

    // The harness's own test process owns no window: reading the list must not make one.
    #[test]
    fn the_harness_itself_owns_no_on_screen_window() {
        assert_eq!(on_screen_windows_of(&[std::process::id()]), 0);
        let _ = frontmost_pid();
        let _ = pasteboard_change_count();
    }
}
