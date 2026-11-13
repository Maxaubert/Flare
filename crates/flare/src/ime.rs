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
use flare_term::handle::CursorStop;

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

/// A tab's cursor stops as the hold reads them: the position, and when on the UI clock, the clock
/// `CaretFollower::on_key` is given. A stop on any other clock never lands in a key's window, so the
/// echo a frame missed would not hold the caret (rule 1206).
pub fn hold_stops(stops: &[CursorStop]) -> Vec<(CaretPos, i64)> {
    stops
        .iter()
        .map(|s| {
            (
                CaretPos {
                    line: i64::from(s.line),
                    x: s.col as i64,
                },
                crate::ui_clock::ms_at(s.at) as i64,
            )
        })
        .collect()
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

    /// Rule 1206: the stops a tab records and the key the window notes are on one clock, so an echo
    /// that only a stop saw still holds the caret on the input line while the stream parks above it.
    #[test]
    fn a_tabs_real_stops_hold_the_caret_after_a_real_key() {
        let mut f = CaretFollower::new();
        f.on_key(crate::ui_clock::monotonic_ms() as i64);
        let now = std::time::Instant::now();
        let stops = [
            CursorStop {
                line: 11,
                col: 12,
                at: now,
            },
            CursorStop {
                line: 3,
                col: 8,
                at: now + std::time::Duration::from_millis(5),
            },
        ];
        f.on_stops(&hold_stops(&stops));
        let frame = crate::ui_clock::monotonic_ms() as i64 + 5;
        assert_eq!(f.on_frame(pos(3, 8), false, frame), pos(11, 12));
        // And after the window closes, the stream's next frame is still held back.
        assert_eq!(f.on_frame(pos(3, 8), false, frame + 600), pos(11, 12));
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
