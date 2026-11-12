//! The terminal's text cursor as accessibility sees it on macOS (issue #201, rule 1187): the Mac counterpart of the
//! Win32 system caret in [`crate::system_caret`]. Zoom's "follow keyboard focus", VoiceOver and magnifiers such as
//! Wind read the insertion point from the accessibility tree: the app's `AXFocusedUIElement`, its
//! `AXSelectedTextRange` and the `AXBoundsForRange` of that range. Flare draws its terminal itself, so it makes one
//! accessibility element for the grid (`ax_terminal` on macOS) and feeds it the cell the IME is given in `place_ime`.
//!
//! This module holds the rules, pure and on every system, so they are tested without a window: when the element
//! claims the focus and which notifications to post ([`AxCaretPlan`]), where a cell of device pixels lands in the
//! view's points ([`view_rect`]), and what the element says its text is ([`caret_line`], [`range_rect`]).

/// The cursor's cell: x, y from the view's top left, and its size, all in device (physical) pixels, as `place_ime`
/// gives the IME.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AxCell {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A notification to post, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxEvent {
    /// The terminal took the keyboard: `NSAccessibilityFocusedUIElementChangedNotification` on the element.
    FocusGained,
    /// The terminal gave the keyboard up (a field, the panel, another window): the same notification, on the view,
    /// so trackers read the new focus.
    FocusLost,
    /// The cursor's cell changed while the terminal has the keyboard: `NSAccessibilitySelectedTextChangedNotification`.
    CaretMoved,
}

/// Whether the terminal claims the accessibility focus, and which notifications that makes. It claims it while the
/// window has the keyboard and a terminal is in front with a cell placed. A frame that leaves the cell where it was
/// posts nothing, so trackers get one event per move, not one per frame.
#[derive(Debug, Default)]
pub struct AxCaretPlan {
    /// The window has the keyboard and the terminal holds it (no field, panel or menu does).
    focused: bool,
    /// The cell last placed; None with no terminal in front.
    cell: Option<AxCell>,
    /// The cell last told to accessibility, while the focus is claimed.
    told: Option<AxCell>,
}

impl AxCaretPlan {
    /// The terminal gained or lost the keyboard.
    pub fn focus(&mut self, on: bool) -> Vec<AxEvent> {
        self.update(on, self.cell)
    }

    /// The cursor's cell, each frame; None when no terminal is in front.
    pub fn place(&mut self, cell: Option<AxCell>) -> Vec<AxEvent> {
        self.update(self.focused, cell)
    }

    /// Both at once (a frame reads the focus too).
    pub fn update(&mut self, focused: bool, cell: Option<AxCell>) -> Vec<AxEvent> {
        self.focused = focused;
        self.cell = cell;
        let now = if focused { cell } else { None };
        match (self.told, now) {
            (None, Some(c)) => {
                self.told = Some(c);
                vec![AxEvent::FocusGained]
            }
            (Some(_), None) => {
                self.told = None;
                vec![AxEvent::FocusLost]
            }
            (Some(was), Some(c)) if was != c => {
                self.told = Some(c);
                vec![AxEvent::CaretMoved]
            }
            _ => Vec::new(),
        }
    }

    /// Whether the terminal claims the focus now.
    pub fn claimed(&self) -> bool {
        self.told.is_some()
    }

    /// The cell last placed, claimed or not.
    pub fn cell(&self) -> Option<AxCell> {
        self.cell
    }
}

/// The scale as used for placing: finite and above 0, else 1.
fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Pure: a cell of device pixels (from the view's top left) as (x, y, w, h) in the view's points, at `scale`
/// (device pixels per point, the window's backing scale). `view_h` is the view's height in points and `flipped`
/// whether its y runs down; AppKit's default view has y up from its bottom left, so there y is the cell's bottom
/// edge measured from the view's bottom. The view then converts it to the window and the window to the screen
/// (Cocoa's screen space: y up from the bottom of the main display), which handles every display and its scale.
pub fn view_rect(cell: AxCell, scale: f64, view_h: f64, flipped: bool) -> (f64, f64, f64, f64) {
    let s = clean_scale(scale);
    let x = f64::from(cell.x) / s;
    let top = f64::from(cell.y) / s;
    let (w, h) = (f64::from(cell.w.max(1)) / s, f64::from(cell.h.max(1)) / s);
    let y = if flipped { top } else { view_h - top - h };
    (x, y, w, h)
}

/// Pure: the element's text and the cursor's index in it (UTF-16 units, as NSRange counts). The text is the
/// cursor's line (`row_text`, trailing blanks trimmed), padded with spaces out to the cursor when it stands past the
/// line's end, so the index is always inside or at the end of the text. A wide character counts as one character,
/// the first of its two cells (`row_text` skips the spacer).
pub fn caret_line(row_text: &str, col: usize) -> (String, usize) {
    let mut text = row_text.to_string();
    let have = text.chars().count();
    if have < col {
        text.extend(std::iter::repeat_n(' ', col - have));
    }
    let index = text.chars().take(col).map(char::len_utf16).sum();
    (text, index)
}

/// Pure: the cell rectangle (device pixels) for the text range starting at `loc` with `len` characters, on the
/// cursor's line whose cursor index is `index` in `caret`. One cell per character to either side of the cursor; an
/// empty range (the insertion point) is the cursor's cell, which is what Zoom and Wind centre on.
pub fn range_rect(caret: AxCell, index: usize, loc: usize, len: usize) -> AxCell {
    let shift = loc as i128 - index as i128;
    let x = i128::from(caret.x) + shift * i128::from(caret.w);
    let w = i128::from(caret.w) * len.max(1) as i128;
    AxCell {
        x: x.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32,
        w: w.clamp(1, i128::from(i32::MAX)) as i32,
        ..caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: AxCell = AxCell {
        x: 40,
        y: 60,
        w: 18,
        h: 36,
    };
    const B: AxCell = AxCell {
        x: 58,
        y: 60,
        w: 18,
        h: 36,
    };

    #[test]
    fn the_focus_is_claimed_only_with_the_keyboard_and_a_cell() {
        let mut p = AxCaretPlan::default();
        assert!(p.focus(true).is_empty(), "no terminal cell yet");
        assert!(!p.claimed());
        assert_eq!(p.place(Some(A)), vec![AxEvent::FocusGained]);
        assert!(p.claimed());
        let mut q = AxCaretPlan::default();
        assert!(q.place(Some(A)).is_empty(), "a cell without the keyboard");
        assert_eq!(q.focus(true), vec![AxEvent::FocusGained]);
    }

    #[test]
    fn a_frame_that_leaves_the_cell_posts_nothing_and_a_move_posts_once() {
        let mut p = AxCaretPlan::default();
        p.update(true, Some(A));
        for _ in 0..5 {
            assert!(p.place(Some(A)).is_empty());
        }
        assert_eq!(p.place(Some(B)), vec![AxEvent::CaretMoved]);
        assert!(p.place(Some(B)).is_empty());
        assert!(p.focus(true).is_empty(), "telling the same focus again");
    }

    #[test]
    fn losing_the_keyboard_or_the_terminal_gives_the_focus_up_once() {
        let mut p = AxCaretPlan::default();
        p.update(true, Some(A));
        assert_eq!(p.focus(false), vec![AxEvent::FocusLost]);
        assert!(p.focus(false).is_empty());
        assert!(p.place(Some(B)).is_empty(), "moves while away post nothing");
        assert_eq!(p.focus(true), vec![AxEvent::FocusGained]);
        assert_eq!(
            p.place(None),
            vec![AxEvent::FocusLost],
            "no terminal in front"
        );
        assert!(!p.claimed());
        assert_eq!(p.cell(), None);
    }

    #[test]
    fn a_cell_becomes_points_in_a_plain_or_flipped_view() {
        // At 2x, a cell 18 x 36 device pixels at (40, 60) is 9 x 18 points at (20, 30) from the top left.
        assert_eq!(view_rect(A, 2.0, 400.0, true), (20.0, 30.0, 9.0, 18.0));
        // In AppKit's default view (y up), its bottom edge is 400 - 30 - 18 = 352 points up.
        assert_eq!(view_rect(A, 2.0, 400.0, false), (20.0, 352.0, 9.0, 18.0));
        // At 1x the same pixels are as many points.
        assert_eq!(view_rect(A, 1.0, 400.0, false), (40.0, 304.0, 18.0, 36.0));
        // At 3x (a magnifier's own zoom changes nothing here; the backing scale does).
        let (x, y, w, h) = view_rect(A, 3.0, 300.0, false);
        assert!((x - 13.333).abs() < 0.001 && (y - 268.0).abs() < 0.001);
        assert!((w - 6.0).abs() < 0.001 && (h - 12.0).abs() < 0.001);
    }

    #[test]
    fn a_bad_scale_places_at_one_and_an_empty_cell_is_one_pixel() {
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                view_rect(A, s, 100.0, true),
                (40.0, 60.0, 18.0, 36.0),
                "at {s}"
            );
        }
        let empty = AxCell {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(view_rect(empty, 1.0, 10.0, false), (0.0, 9.0, 1.0, 1.0));
    }

    #[test]
    fn the_text_is_the_cursor_line_and_the_index_counts_utf16() {
        assert_eq!(caret_line("$ ls", 4), ("$ ls".to_string(), 4));
        assert_eq!(caret_line("$ ls", 2), ("$ ls".to_string(), 2));
        // Past the trimmed end: padded out to the cursor.
        assert_eq!(caret_line("$", 3), ("$  ".to_string(), 3));
        assert_eq!(caret_line("", 0), (String::new(), 0));
        // A character outside the BMP is two UTF-16 units.
        assert_eq!(caret_line("a\u{1F600}b", 2).1, 3);
    }

    #[test]
    fn a_range_is_cells_beside_the_cursor_and_the_insertion_point_is_its_cell() {
        assert_eq!(range_rect(A, 4, 4, 0), A);
        assert_eq!(range_rect(A, 4, 5, 0), B);
        let back = range_rect(A, 4, 2, 2);
        assert_eq!((back.x, back.w), (40 - 36, 36));
        assert_eq!(
            range_rect(A, 0, usize::MAX, 1).x,
            i32::MAX,
            "clamped, never wraps"
        );
    }
}
