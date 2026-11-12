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
    CreateCaret, DestroyCaret, HideCaret, SetCaretPos,
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
