//! The Win32 system caret: where the typing point is, for Windows Magnifier, Narrator, Wind and the other tools that
//! follow it (issue #70). Flare draws its own cursor, so the caret is made hidden; Windows still reports its place
//! (`GetGUIThreadInfo`'s `rcCaret`) and fires the `OBJID_CARET` location events itself.
//!
//! The rules live in [`CaretPlan`], which only says which calls to make, so they are tested without a window. The
//! calls run on the thread that owns the window (the UI thread): a caret belongs to a thread, and only the window with
//! the keyboard focus has one.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateCaret, DestroyCaret, GUITHREADINFO, GetGUIThreadInfo, HideCaret, SetCaretPos,
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
/// it was makes none. "Focused" means the terminal holds the keyboard, not only the window.
#[derive(Debug, Default)]
pub struct CaretPlan {
    focused: bool,
    /// The cell's top-left and height, as last placed (kept while the window is away, for its return); none when
    /// there is no cell to report.
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
        self.destroy()
    }

    /// One frame: whether the terminal holds the keyboard, and the cursor's cell as (`x`, `y`, height) in the window's
    /// client area (physical pixels), or none when there is no cell to report.
    pub fn frame(&mut self, focused: bool, cell: Option<(i32, i32, i32)>) -> Vec<CaretOp> {
        self.want = cell.map(|(x, y, h)| (x, y, h.max(1)));
        self.focus(focused)
    }

    fn destroy(&mut self) -> Vec<CaretOp> {
        self.at = None;
        match self.made.take() {
            Some(_) => vec![CaretOp::Destroy],
            None => Vec::new(),
        }
    }

    fn sync(&mut self) -> Vec<CaretOp> {
        let Some((x, y, h)) = self.want else {
            return self.destroy();
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
pub fn has_focus(hwnd: isize) -> bool {
    // SAFETY: a plain query of the calling thread's focus.
    unsafe { GetFocus() as isize == hwnd }
}

/// The caret this thread reports, as a magnifier reads it (`GetGUIThreadInfo`): its window and its rectangle (left,
/// top, right, bottom) in that window's client area.
pub fn reported() -> Option<(isize, (i32, i32, i32, i32))> {
    // SAFETY: a zeroed GUITHREADINFO with its size set is what the call asks for.
    unsafe {
        let mut g: GUITHREADINFO = std::mem::zeroed();
        g.cbSize = size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(GetCurrentThreadId(), &mut g) == 0 || g.hwndCaret.is_null() {
            return None;
        }
        let r = g.rcCaret;
        Some((g.hwndCaret as isize, (r.left, r.top, r.right, r.bottom)))
    }
}

/// Makes the planned calls for `hwnd`. Call on the thread that owns the window.
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

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Option<(i32, i32, i32)> = Some((10, 20, 18));

    #[test]
    fn nothing_is_made_before_the_window_has_the_focus() {
        let mut p = CaretPlan::default();
        assert!(p.frame(false, CELL).is_empty());
        assert_eq!(
            p.frame(true, CELL),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 10, y: 20 }]
        );
    }

    #[test]
    fn a_focus_with_no_cell_yet_waits_for_the_first_frame() {
        let mut p = CaretPlan::default();
        assert!(p.focus(true).is_empty());
        assert_eq!(
            p.frame(true, Some((1, 2, 18))),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 1, y: 2 }]
        );
    }

    #[test]
    fn a_frame_that_leaves_the_cursor_makes_no_call() {
        let mut p = CaretPlan::default();
        p.frame(true, CELL);
        assert!(p.frame(true, CELL).is_empty());
        assert_eq!(
            p.frame(true, Some((19, 20, 18))),
            vec![CaretOp::Move { x: 19, y: 20 }]
        );
    }

    #[test]
    fn a_new_cell_height_makes_the_caret_again() {
        let mut p = CaretPlan::default();
        p.frame(true, CELL);
        assert_eq!(
            p.frame(true, Some((10, 20, 27))),
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
        p.frame(true, CELL);
        assert_eq!(p.focus(false), vec![CaretOp::Destroy]);
        assert!(
            p.frame(false, Some((30, 40, 18))).is_empty(),
            "no caret while away"
        );
        assert!(p.focus(false).is_empty(), "nothing left to destroy");
        assert_eq!(
            p.focus(true),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 30, y: 40 }]
        );
    }

    #[test]
    fn telling_the_same_focus_again_makes_no_call() {
        let mut p = CaretPlan::default();
        p.frame(true, CELL);
        assert!(p.focus(true).is_empty());
        p.focus(false);
        assert!(p.focus(false).is_empty());
    }

    #[test]
    fn a_zero_height_is_one_pixel() {
        let mut p = CaretPlan::default();
        assert_eq!(p.frame(true, Some((0, 0, 0)))[0], CaretOp::Create { h: 1 });
    }

    #[test]
    fn a_frame_with_no_cell_destroys_it_and_a_cell_makes_it_again() {
        let mut p = CaretPlan::default();
        p.frame(true, CELL);
        assert_eq!(p.frame(true, None), vec![CaretOp::Destroy]);
        assert!(p.frame(true, None).is_empty(), "nothing left to destroy");
        assert_eq!(
            p.frame(true, CELL),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 10, y: 20 }]
        );
    }

    #[test]
    fn a_frame_without_the_keys_destroys_it_and_no_stale_place_is_made_on_return() {
        let mut p = CaretPlan::default();
        p.frame(true, CELL);
        assert_eq!(p.frame(false, CELL), vec![CaretOp::Destroy]);
        assert_eq!(
            p.frame(true, Some((40, 20, 18))),
            vec![CaretOp::Create { h: 18 }, CaretOp::Move { x: 40, y: 20 }],
            "one move, to the new cell"
        );
    }

    /// A hidden popup of a system class, owned by this test's thread.
    fn test_window() -> HWND {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_POPUP};
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a plain window creation; the caller destroys it.
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
        hwnd
    }

    #[test]
    fn the_calls_report_a_thin_hidden_caret_where_the_cell_is() {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let window = test_window();
        let hwnd = window as isize;
        let mut p = CaretPlan::default();
        apply(hwnd, &p.frame(true, Some((5, 7, 18))));
        assert_eq!(reported(), Some((hwnd, (5, 7, 5 + CARET_W, 25))));
        apply(hwnd, &p.frame(true, Some((22, 7, 18))));
        assert_eq!(
            reported().map(|r| r.1),
            Some((22, 7, 22 + CARET_W, 25)),
            "it follows the cursor"
        );
        apply(hwnd, &p.focus(false));
        assert_eq!(reported(), None, "no caret once the focus is gone");
        // SAFETY: the window made above, on this thread.
        unsafe {
            DestroyWindow(window);
        }
    }

    thread_local! {
        /// The caret location events this thread's hook has received.
        static CARET_EVENTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    unsafe extern "system" fn count_caret_event(
        _hook: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
        _event: u32,
        _hwnd: HWND,
        object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if object == windows_sys::Win32::UI::WindowsAndMessaging::OBJID_CARET {
            CARET_EVENTS.with(|c| c.set(c.get() + 1));
        }
    }

    #[test]
    fn a_move_tells_windows_listeners_even_though_the_caret_is_hidden() {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DestroyWindow, DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, MSG, PM_REMOVE,
            PeekMessageW, WINEVENT_OUTOFCONTEXT,
        };
        let window = test_window();
        let hwnd = window as isize;
        // SAFETY: an out-of-context hook for this process and thread, removed below; the callback only counts.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                std::ptr::null_mut(),
                Some(count_caret_event),
                GetCurrentProcessId(),
                GetCurrentThreadId(),
                WINEVENT_OUTOFCONTEXT,
            )
        };
        assert!(!hook.is_null(), "a hook");
        let mut p = CaretPlan::default();
        apply(hwnd, &p.frame(true, Some((5, 7, 18))));
        apply(hwnd, &p.frame(true, Some((22, 7, 18))));
        // Out-of-context events arrive through this thread's message queue.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while CARET_EVENTS.with(std::cell::Cell::get) == 0 && std::time::Instant::now() < until {
            // SAFETY: a zeroed MSG for PeekMessageW to fill, on this thread's own queue.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::yield_now();
        }
        let seen = CARET_EVENTS.with(std::cell::Cell::get);
        apply(hwnd, &p.focus(false));
        // SAFETY: the hook and window made above, on this thread.
        unsafe {
            UnhookWinEvent(hook);
            DestroyWindow(window);
        }
        assert!(seen > 0, "no caret location event reached the hook");
    }
}
