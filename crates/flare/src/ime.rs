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

/// The grid cell (row, column) the system caret reports for the cursor or its hold. A scrolled-back view shows the
/// cursor lower down, and has left it behind altogether once it is below the last row: then there is no cell.
pub fn caret_cell(held: CaretPos, display_offset: usize, rows: usize) -> Option<(usize, usize)> {
    let row = held.line.max(0) as usize + display_offset;
    (row < rows).then_some((row, held.x.max(0) as usize))
}

/// The system caret's top-left and height in window device pixels: the same cell the IME is given.
pub fn caret_rect(
    cell: Option<(usize, usize)>,
    m: &CellMetrics,
    grid_origin_phys: (i32, i32),
) -> Option<(i32, i32, i32)> {
    let (row, col) = cell?;
    let a = ime_area(row, col, m, grid_origin_phys);
    Some((a.x, a.y, a.h as i32))
}

/// Whether the keyboard is the terminal's: the window has the focus, a terminal is in front (not Settings or the start
/// screen) and no field, dialog or popup holds the keys (`hollow`).
pub fn terminal_holds_keys(window_focus: bool, front_is_terminal: bool, hollow: bool) -> bool {
    window_focus && front_is_terminal && !hollow
}

thread_local! {
    /// The rectangle last handed to the window (read by the e2e ime.rect probe).
    static LAST_AREA: std::cell::Cell<Option<ImeArea>> = const { std::cell::Cell::new(None) };
}

/// Remembers the rectangle just given to the window's input method.
pub fn record_area(a: ImeArea) {
    LAST_AREA.with(|c| c.set(Some(a)));
}

/// The rectangle last given to the window's input method, if a frame has placed one.
pub fn last_area() -> Option<ImeArea> {
    LAST_AREA.with(std::cell::Cell::get)
}

/// The caret hold as the window drives it: a key opens the window, each frame reports the cursor.
#[derive(Clone, Copy, Debug)]
pub struct CaretFollower {
    state: CaretHoldState,
    key_ms: i64,
}

impl CaretFollower {
    pub fn new() -> Self {
        Self {
            state: initial_caret_hold(),
            key_ms: i64::MIN,
        }
    }

    /// A key was typed at `now_ms`: the 250 ms hold window opens.
    pub fn on_key(&mut self, now_ms: i64) {
        self.state = on_caret_key(&self.state, now_ms);
        self.key_ms = now_ms;
    }

    /// The positions the cursor stopped at while output was parsed since the last frame (the
    /// echo of a key can end on the input line and be overwritten before any frame shows it).
    /// Each stop made after the last key is taken as a write's end, in order.
    pub fn on_stops(&mut self, stops: &[(CaretPos, i64)]) {
        for &(pos, at_ms) in stops {
            if at_ms >= self.key_ms {
                let (next, _) = on_caret_move(&self.state, pos, at_ms);
                self.state = next;
            }
        }
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
    fn the_last_area_given_to_the_window_is_kept_for_the_probe() {
        let a = ImeArea {
            x: 5,
            y: 7,
            w: 9,
            h: 11,
        };
        record_area(a);
        assert_eq!(last_area(), Some(a));
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
    fn the_system_caret_is_the_ime_cell_at_every_scale_and_origin() {
        for scale in SCALES {
            let m = cell_metrics(&cascadia(), scale);
            for origin in [(0, 0), (37, 61), (412, 96)] {
                let a = ime_area(4, 11, &m, origin);
                assert_eq!(
                    caret_rect(Some((4, 11)), &m, origin),
                    Some((a.x, a.y, a.h as i32)),
                    "scale {scale} origin {origin:?}"
                );
            }
        }
    }

    #[test]
    fn a_sidebar_or_a_taller_strip_moves_the_system_caret_with_the_grid() {
        let m = cell_metrics(&cascadia(), 1.5);
        let (x0, y0, _) = caret_rect(Some((2, 3)), &m, (0, 0)).unwrap();
        let (x1, y1, _) = caret_rect(Some((2, 3)), &m, (240, 48)).unwrap();
        assert_eq!((x1 - x0, y1 - y0), (240, 48));
    }

    #[test]
    fn no_cell_means_no_system_caret() {
        let m = cell_metrics(&cascadia(), 1.0);
        assert_eq!(caret_rect(None, &m, (10, 10)), None);
    }

    #[test]
    fn a_scrolled_back_view_moves_the_cursor_down_and_then_loses_it() {
        assert_eq!(caret_cell(pos(10, 4), 0, 24), Some((10, 4)));
        assert_eq!(caret_cell(pos(10, 4), 13, 24), Some((23, 4)));
        assert_eq!(caret_cell(pos(10, 4), 14, 24), None);
    }

    #[test]
    fn the_terminal_holds_the_keys_only_with_the_focus_in_front_and_nothing_over_it() {
        assert!(terminal_holds_keys(true, true, false));
        assert!(!terminal_holds_keys(false, true, false), "another window");
        assert!(
            !terminal_holds_keys(true, false, false),
            "Settings or the start screen"
        );
        assert!(
            !terminal_holds_keys(true, true, true),
            "a field, dialog or popup"
        );
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
    fn an_echo_stop_no_frame_saw_still_holds_the_caret() {
        let mut f = CaretFollower::new();
        f.on_key(1000);
        // The echo ended on the input line, then the stream parked the cursor above it, and
        // only the second position ever reached a frame.
        f.on_stops(&[(pos(24, 4), 1010), (pos(10, 3), 1020)]);
        assert_eq!(f.on_frame(pos(10, 3), false, 1030), pos(24, 4));
    }

    #[test]
    fn a_stop_from_before_the_key_is_not_its_echo() {
        let mut f = CaretFollower::new();
        f.on_key(1000);
        f.on_stops(&[(pos(24, 4), 990)]);
        assert_eq!(f.on_frame(pos(10, 3), false, 1010), pos(10, 3));
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
