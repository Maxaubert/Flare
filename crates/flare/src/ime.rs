//! The IME caret hold and the IME area (Task 24b): where the input method's candidate window goes.
//!
//! `ime_area` is the cell under the caret in window device pixels. `CaretFollower` holds the caret
//! on the input line for a short window after a key (flare-term's `caret_hold`, Prism Terminal's
//! termCaretHold.ts), so streaming output that parks the cursor above the prompt does not pull the
//! candidate window away. The hold is released on the alternate screen, a resize and a scrolled-back
//! view.

use flare_render::metrics::{CellMetrics, cell_origin};
use flare_term::caret_hold::{
    CaretHoldState, CaretPos, initial_caret_hold, on_caret_key, on_caret_move, release_caret_hold,
};

/// The text-input rectangle the IME is told about, in window device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImeArea {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// The cell at (`row`, `col`) of the grid, in window device pixels: the cell's origin (padding
/// included) plus the grid's origin in the window.
pub fn ime_area(row: usize, col: usize, m: &CellMetrics, grid_origin_phys: (i32, i32)) -> ImeArea {
    let (x, y) = cell_origin(col, row, m);
    ImeArea {
        x: x + grid_origin_phys.0,
        y: y + grid_origin_phys.1,
        w: m.cell_w,
        h: m.cell_h,
    }
}

/// The caret hold as the window drives it: a key opens the window, each frame reports the cursor.
#[derive(Clone, Copy, Debug)]
pub struct CaretFollower {
    state: CaretHoldState,
}

impl CaretFollower {
    pub fn new() -> Self {
        Self {
            state: initial_caret_hold(),
        }
    }

    /// A key was typed at `now_ms`: the 250 ms hold window opens.
    pub fn on_key(&mut self, now_ms: i64) {
        self.state = on_caret_key(&self.state, now_ms);
    }

    /// Each frame: the position the IME rectangle goes to. `release` is true on the alternate
    /// screen, after a resize, or on a scrolled-back view: the hold is let go and the real cursor wins.
    pub fn on_frame(&mut self, cursor: CaretPos, release: bool, now_ms: i64) -> CaretPos {
        if release {
            self.state = release_caret_hold();
            return cursor;
        }
        let (next, hold) = on_caret_move(&self.state, cursor, now_ms);
        self.state = next;
        hold.unwrap_or(cursor)
    }
}

impl Default for CaretFollower {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::metrics::{FontUnits, cell_metrics};

    const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn cascadia() -> FontUnits {
        FontUnits {
            units_per_em: 2048.0,
            ascent: 1900.0,
            descent: -480.0,
            leading: 0.0,
            advance: 1200.0,
        }
    }

    fn pos(line: i64, x: i64) -> CaretPos {
        CaretPos { line, x }
    }

    #[test]
    fn the_area_is_the_cell_at_every_scale() {
        let origin = (37, 61);
        for scale in SCALES {
            let m = cell_metrics(&cascadia(), scale);
            let a = ime_area(2, 3, &m, origin);
            assert_eq!(
                a.x,
                m.padding as i32 + 3 * m.cell_w as i32 + origin.0,
                "scale {scale}"
            );
            assert_eq!(
                a.y,
                m.padding as i32 + 2 * m.cell_h as i32 + origin.1,
                "scale {scale}"
            );
            assert_eq!(a.w, m.cell_w, "scale {scale}");
            assert_eq!(a.h, m.cell_h, "scale {scale}");
        }
    }

    #[test]
    fn streaming_above_the_input_line_keeps_the_caret_on_it() {
        let mut f = CaretFollower::new();
        f.on_key(1000);
        assert_eq!(f.on_frame(pos(24, 4), false, 1010), pos(24, 4));
        // Output parks the cursor higher up while the window is open.
        assert_eq!(f.on_frame(pos(10, 3), false, 1050), pos(24, 4));
        // And after the window closes, a cursor still above the held line is held back.
        assert_eq!(f.on_frame(pos(10, 3), false, 1400), pos(24, 4));
    }

    #[test]
    fn a_cursor_at_or_below_the_held_one_re_anchors() {
        let mut f = CaretFollower::new();
        f.on_key(1000);
        assert_eq!(f.on_frame(pos(24, 4), false, 1010), pos(24, 4));
        assert_eq!(f.on_frame(pos(25, 0), false, 1100), pos(25, 0));
        assert_eq!(f.on_frame(pos(26, 0), false, 2000), pos(26, 0));
    }

    #[test]
    fn the_hold_is_released_on_the_alternate_screen_a_resize_and_scrollback() {
        let mut f = CaretFollower::new();
        f.on_key(1000);
        assert_eq!(f.on_frame(pos(24, 4), false, 1010), pos(24, 4));
        // The release flag (alternate screen, resize or scrollback) returns the real cursor...
        assert_eq!(f.on_frame(pos(10, 3), true, 1020), pos(10, 3));
        // ...and forgets the hold: a later higher cursor is no longer held back.
        assert_eq!(f.on_frame(pos(5, 0), false, 1030), pos(5, 0));
    }

    #[test]
    fn without_a_key_there_is_no_hold() {
        let mut f = CaretFollower::new();
        assert_eq!(f.on_frame(pos(24, 4), false, 1000), pos(24, 4));
        assert_eq!(f.on_frame(pos(10, 3), false, 1100), pos(10, 3));
    }
}
