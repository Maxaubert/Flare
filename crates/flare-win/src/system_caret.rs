//! The Win32 system caret: where the typing point is, for Windows Magnifier, Narrator, Wind and the other tools that
//! follow it (issue #70). Flare draws its own cursor, so the caret is made hidden; Windows still reports its place
//! (`GetGUIThreadInfo`'s `rcCaret`) and fires the `OBJID_CARET` location events itself.
//!
//! The rules live in [`CaretPlan`], which only says which calls to make, so they are tested without a window. The
//! calls run on the thread that owns the window (the UI thread): a caret belongs to a thread, and only the window with
//! the keyboard focus has one.

#[cfg(windows)]
use windows_sys::Win32::Foundation::HWND;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateCaret, DestroyCaret, GetCaretBlinkTime, HideCaret, SetCaretPos,
};

/// The caret's width in physical pixels. Thin on purpose: a tool may ignore a caret as wide as a whole cell or line.
pub const CARET_W: i32 = 2;

/// One call to make, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretOp {
    /// `CreateCaret` with this height (and [`CARET_W`]), then `HideCaret`.
    Create { h: i32 },
    /// `SetCaretPos`, in the window's client area, physical pixels.
    Move { x: i32, y: i32 },
    /// `DestroyCaret`.
    Destroy,
}

/// What the caret is and where it is wanted. Calls are planned on a change only: a frame that leaves the cursor where
/// it was makes none.
#[derive(Debug, Default)]
pub struct CaretPlan {
    focused: bool,
    /// The cell's top-left and height, as last placed (kept while the window is away, for its return).
    want: Option<(i32, i32, i32)>,
    /// The height of the caret that exists now, if one does.
    made: Option<i32>,
    /// Where the existing caret was last moved.
    at: Option<(i32, i32)>,
}

impl CaretPlan {
    /// The window gained or lost the keyboard focus.
    pub fn focus(&mut self, on: bool) -> Vec<CaretOp> {
        self.focused = on;
        if on {
            return self.sync();
        }
        let mut ops = Vec::new();
        if self.made.take().is_some() {
            ops.push(CaretOp::Destroy);
        }
        self.at = None;
        ops
    }

    /// The cursor's cell is at (`x`, `y`), `h` tall, in the window's client area (physical pixels).
    pub fn place(&mut self, x: i32, y: i32, h: i32) -> Vec<CaretOp> {
        self.want = Some((x, y, h.max(1)));
        if !self.focused {
            return Vec::new();
        }
        self.sync()
    }

    fn sync(&mut self) -> Vec<CaretOp> {
        let Some((x, y, h)) = self.want else {
            return Vec::new();
        };
        let mut ops = Vec::new();
        if self.made != Some(h) {
            if self.made.is_some() {
                ops.push(CaretOp::Destroy);
            }
            ops.push(CaretOp::Create { h });
            self.made = Some(h);
            self.at = None;
        }
        if self.at != Some((x, y)) {
            ops.push(CaretOp::Move { x, y });
            self.at = Some((x, y));
        }
        ops
    }
}

/// True when `hwnd` has the keyboard focus of the calling thread (the UI thread, for Flare's window).
#[cfg(windows)]
pub fn has_focus(hwnd: isize) -> bool {
    // SAFETY: a plain query of the calling thread's focus.
    unsafe { GetFocus() as isize == hwnd }
}

/// Makes the planned calls for `hwnd`. Call on the thread that owns the window.
#[cfg(windows)]
pub fn apply(hwnd: isize, ops: &[CaretOp]) {
    for op in ops {
        // SAFETY: plain calls on the caller's own window and thread; a stale handle or a missing caret just fails.
        unsafe {
            match *op {
                CaretOp::Create { h } => {
                    if CreateCaret(hwnd as HWND, std::ptr::null_mut(), CARET_W, h) != 0 {
                        HideCaret(hwnd as HWND);
                    }
                }
                CaretOp::Move { x, y } => {
                    SetCaretPos(x, y);
                }
                CaretOp::Destroy => {
                    DestroyCaret();
                }
            }
        }
    }
}

/// macOS: false, on purpose. The Mac has no system caret to make: Zoom's "follow keyboard focus"
/// and VoiceOver read the insertion point from the accessibility tree (the focused element's
/// `AXSelectedTextRange` and `AXBoundsForRange`), so the Mac counterpart is an accessibility element
/// for the terminal: `crate::ax_terminal` (rule 1187), fed the same cell from `place_ime`. With no
/// focus reported here the plan never asks for a call.
#[cfg(not(windows))]
pub fn has_focus(_hwnd: isize) -> bool {
    false
}

/// macOS: nothing to drive (see `has_focus`); the plan's calls are dropped.
#[cfg(not(windows))]
pub fn apply(_hwnd: isize, _ops: &[CaretOp]) {}

/// The person's caret blink time in milliseconds (`GetCaretBlinkTime`, Control Panel's cursor blink rate), for
/// Flare's own drawn caret (rule 1233). `u32::MAX` (INFINITE) means "do not blink"; 0 means the call failed.
#[cfg(windows)]
pub fn blink_time_ms() -> Option<u32> {
    // SAFETY: a plain query of a system setting; it takes no arguments.
    Some(unsafe { GetCaretBlinkTime() })
}

/// A macOS blink period at or above this many milliseconds is read as "do not blink". The Mac has no switch for a
/// steady caret: the way people turn the blink off is a huge `NSTextInsertionPointBlinkPeriodOn` (often 999999 or
/// more). 30 s is far past any period anyone would want to see tick, and far past `caret_blink`'s 5 s ceiling, so a
/// value this large is meant as "steady", not as a slow blink (rule 1234).
pub const MAC_STEADY_MS: f64 = 30_000.0;

/// Pure: the two macOS blink defaults (`NSTextInsertionPointBlinkPeriodOn` and `...Off`, milliseconds, as
/// `doubleForKey` gives them: 0 when unset) to the same raw value `GetCaretBlinkTime` gives on Windows. The on period
/// wins, then the off period; neither set (0, negative or not a number) is `None`, so the caller ticks at its
/// default. [`MAC_STEADY_MS`] or more is `u32::MAX`, Windows' INFINITE, "do not blink".
pub fn mac_blink_ms(on: f64, off: f64) -> Option<u32> {
    let usable = |v: f64| v.is_finite() && v > 0.0;
    let ms = [on, off].into_iter().find(|&v| usable(v))?;
    if ms >= MAC_STEADY_MS {
        return Some(u32::MAX);
    }
    // Below 30 000, so the rounded value always fits.
    Some(ms.round() as u32)
}

/// The person's caret blink period on macOS, for Flare's own drawn caret (rule 1234): the global defaults
/// `NSTextInsertionPointBlinkPeriodOn` (then `NSTextInsertionPointBlinkPeriodOff`), which AppKit's own text views
/// follow. Unset (the usual case) is `None` and the caller ticks at its default; see [`mac_blink_ms`].
#[cfg(target_os = "macos")]
pub fn blink_time_ms() -> Option<u32> {
    use objc2_foundation::{NSString, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    let read = |key: &str| defaults.doubleForKey(&NSString::from_str(key));
    mac_blink_ms(
        read("NSTextInsertionPointBlinkPeriodOn"),
        read("NSTextInsertionPointBlinkPeriodOff"),
    )
}

/// Other systems: no blink setting is read, so the caller ticks at its default.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn blink_time_ms() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_made_before_the_window_has_the_focus() {
        let mut p = CaretPlan::default();
        assert!(p.place(10, 20, 18).is_empty());
        assert_eq!(
            p.focus(true),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 10, y: 20 }]
        );
    }

    #[test]
    fn a_focus_with_no_place_yet_waits_for_the_first_frame() {
        let mut p = CaretPlan::default();
        assert!(p.focus(true).is_empty());
        assert_eq!(
            p.place(1, 2, 18),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 1, y: 2 }]
        );
    }

    #[test]
    fn a_frame_that_leaves_the_cursor_makes_no_call() {
        let mut p = CaretPlan::default();
        p.focus(true);
        p.place(10, 20, 18);
        assert!(p.place(10, 20, 18).is_empty());
        assert_eq!(p.place(19, 20, 18), vec![CaretOp::Move { x: 19, y: 20 }]);
    }

    #[test]
    fn a_new_cell_height_makes_the_caret_again() {
        let mut p = CaretPlan::default();
        p.focus(true);
        p.place(10, 20, 18);
        assert_eq!(
            p.place(10, 20, 27),
            vec![
                CaretOp::Destroy,
                CaretOp::Create { h: 27 },
                CaretOp::Move { x: 10, y: 20 }
            ]
        );
    }

    #[test]
    fn losing_the_focus_destroys_it_and_coming_back_puts_it_where_the_cursor_went() {
        let mut p = CaretPlan::default();
        p.focus(true);
        p.place(10, 20, 18);
        assert_eq!(p.focus(false), vec![CaretOp::Destroy]);
        assert!(p.place(30, 40, 18).is_empty(), "no caret while away");
        assert!(p.focus(false).is_empty(), "nothing left to destroy");
        assert_eq!(
            p.focus(true),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 30, y: 40 }]
        );
    }

    #[test]
    fn telling_the_same_focus_again_makes_no_call() {
        let mut p = CaretPlan::default();
        p.focus(true);
        p.place(10, 20, 18);
        assert!(p.focus(true).is_empty());
        p.focus(false);
        assert!(p.focus(false).is_empty());
    }

    #[test]
    fn a_zero_height_is_one_pixel() {
        let mut p = CaretPlan::default();
        p.focus(true);
        assert_eq!(p.place(0, 0, 0)[0], CaretOp::Create { h: 1 });
    }

    #[test]
    fn the_mac_blink_period_comes_from_the_on_default_then_the_off_default() {
        assert_eq!(mac_blink_ms(0.0, 0.0), None, "unset: the default tick");
        assert_eq!(mac_blink_ms(400.0, 0.0), Some(400));
        assert_eq!(mac_blink_ms(400.0, 700.0), Some(400), "the on period wins");
        assert_eq!(mac_blink_ms(0.0, 700.0), Some(700), "then the off period");
        assert_eq!(
            mac_blink_ms(-5.0, 650.4),
            Some(650),
            "a negative value is unset"
        );
        assert_eq!(
            mac_blink_ms(f64::NAN, f64::INFINITY),
            None,
            "not a number is unset"
        );
    }

    #[test]
    fn a_huge_mac_blink_period_is_steady() {
        assert_eq!(mac_blink_ms(MAC_STEADY_MS, 0.0), Some(u32::MAX));
        assert_eq!(mac_blink_ms(999_999_999.0, 500.0), Some(u32::MAX));
        assert_eq!(mac_blink_ms(MAC_STEADY_MS - 1.0, 0.0), Some(29_999));
        assert_eq!(
            mac_blink_ms(0.0, 1e12),
            Some(u32::MAX),
            "a huge off period alone too"
        );
    }

    /// The real defaults read runs headless and gives what the pure mapping allows.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_defaults_read_runs_without_a_window() {
        let got = blink_time_ms();
        assert!(
            got.is_none_or(|ms| ms == u32::MAX || ms < 30_000),
            "{got:?}"
        );
    }

    /// What a magnifier reads of this thread: the caret's window and rectangle (`GetGUIThreadInfo`).
    #[cfg(windows)]
    fn read_caret() -> (isize, (i32, i32, i32, i32)) {
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};
        // SAFETY: a zeroed GUITHREADINFO with its size set is what the call asks for.
        unsafe {
            let mut g: GUITHREADINFO = std::mem::zeroed();
            g.cbSize = size_of::<GUITHREADINFO>() as u32;
            assert_ne!(GetGUIThreadInfo(GetCurrentThreadId(), &mut g), 0);
            let r = g.rcCaret;
            (g.hwndCaret as isize, (r.left, r.top, r.right, r.bottom))
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_calls_report_a_thin_hidden_caret_where_the_cell_is() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WS_POPUP,
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a hidden popup of a system class, owned by this test's thread and destroyed below.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "a test window");
        let hwnd = hwnd as isize;
        let mut p = CaretPlan::default();
        p.focus(true);
        apply(hwnd, &p.place(5, 7, 18));
        assert_eq!(read_caret(), (hwnd, (5, 7, 5 + CARET_W, 25)));
        apply(hwnd, &p.place(22, 7, 18));
        assert_eq!(
            read_caret().1,
            (22, 7, 22 + CARET_W, 25),
            "it follows the cursor"
        );
        apply(hwnd, &p.focus(false));
        assert_eq!(read_caret().0, 0, "no caret once the focus is gone");
        // SAFETY: the window made above, on this thread.
        unsafe {
            DestroyWindow(hwnd as HWND);
        }
    }
}
