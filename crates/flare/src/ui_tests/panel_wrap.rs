//! Wrapping in the text preview (Surfaces, 2026-10-22), headless: the view reports the columns that fit, the controller
//! wraps to them (rows and line numbers handed over as the glue does), a long line takes several rows, the gutter
//! shows a number on a line's first row only, the rows stay virtual and the wrap button calls back. At 100% and 225%.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use slint::{ModelRc, SharedString, VecModel};

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::panel_ctl::wrap_lines;
use crate::ui_harness::Harness;

const SCALES2: [f32; 2] = [1.0, 2.25];
const W: u32 = 1000;
const H: u32 = 640;

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, W, H, || AppWindow::new().expect("the window builds"));
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// Shows `lines` the way the glue does with wrapping on: wrapped to the columns the view reports.
fn show_wrapped(h: &Harness<AppWindow>, lines: &[SharedString]) -> (usize, usize) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(2);
    h.comp.set_panel_number_count(lines.len() as i32);
    h.advance(0);
    let cols = h.comp.get_panel_wrap_cols() as usize;
    let (rows, nums) = wrap_lines(lines, cols, &|| false);
    let n = rows.len();
    h.comp.set_panel_lines(model(rows));
    h.comp.set_panel_line_nums(model(nums));
    h.comp.set_panel_wrapped(true);
    h.advance(0);
    (cols, n)
}

fn long_line() -> SharedString {
    // 500 characters with spaces, so it breaks between words.
    "word ".repeat(100).trim_end().into()
}

#[test]
fn the_view_reports_the_columns_that_fit_beside_the_gutter() {
    for s in SCALES2 {
        let h = window(s);
        h.comp.set_panel_open(true);
        h.comp.set_panel_kind(2);
        h.advance(0);
        let cols = h.comp.get_panel_wrap_cols();
        assert!(
            cols >= 20,
            "scale {s}: a 1000 css px window holds {cols} columns in its slot"
        );
        // A wider share holds more columns; the gutter takes its part.
        h.comp.set_panel_share(0.6);
        h.advance(0);
        assert!(
            h.comp.get_panel_wrap_cols() > cols,
            "scale {s}: wider, more columns"
        );
        // The gutter widens with the line count, so fewer columns fit beside it.
        let wide = h.comp.get_panel_wrap_cols();
        h.comp.set_panel_number_count(200_000);
        h.advance(0);
        assert!(
            h.comp.get_panel_wrap_cols() < wide,
            "scale {s}: a wider gutter leaves fewer columns"
        );
    }
}

#[test]
fn a_500_character_line_wraps_into_several_rows_and_none_is_wider_than_the_columns() {
    for s in SCALES2 {
        let h = window(s);
        let lines = vec![
            SharedString::from("first"),
            long_line(),
            SharedString::from("last"),
        ];
        let (cols, n) = show_wrapped(&h, &lines);
        let per_line = 500usize.div_ceil(cols);
        assert!(
            n >= 1 + per_line.saturating_sub(1) + 2,
            "scale {s}: {n} rows at {cols} columns"
        );
        assert!(
            n > 3,
            "scale {s}: the long line took more than one row ({n})"
        );
        // The view counts rows, not lines.
        let rows = h.comp.get_panel_lines();
        let (want, _) = wrap_lines(&lines, cols, &|| false);
        assert_eq!(slint::Model::row_count(&rows), want.len());
        assert!(
            want.iter().all(|r| r.chars().count() <= cols),
            "scale {s}: every row fits the columns"
        );
        // The rows are drawn: the long line's continuation shows ink on the third visual row.
        let f = h.render();
        let k = s;
        let row_h = (18.0 * k).round() as u32;
        let y0 = ((h.comp.get_panel_body_y()) * k).round() as u32;
        let x0 = ((h.comp.get_panel_body_x() + h.comp.get_panel_gutter_w()) * k).round() as u32;
        let x1 = ((h.comp.get_panel_body_x() + h.comp.get_panel_body_w()) * k).round() as u32;
        for i in 0..4u32 {
            let ink = f.count_where(x0, y0 + i * row_h, x1, y0 + (i + 1) * row_h, |p| {
                p[0] > 90 && p[1] > 90 && p[2] > 90
            });
            assert!(ink > 8, "scale {s}: visual row {i} shows text ({ink})");
        }
    }
}

#[test]
fn nothing_is_drawn_past_the_right_edge_of_the_slot_when_wrapped() {
    for s in SCALES2 {
        let h = window(s);
        let lines = vec![long_line(); 6];
        show_wrapped(&h, &lines);
        let f = h.render();
        // The slot ends at the window's right edge; the strip between the last column and the thumb is clear.
        let right = ((h.comp.get_panel_body_x() + h.comp.get_panel_body_w()) * s).round() as u32;
        let y0 = (h.comp.get_panel_body_y() * s).round() as u32;
        let strip = f.count_where(
            right.saturating_sub((8.0 * s) as u32),
            y0,
            right.saturating_sub((4.0 * s) as u32),
            y0 + (6.0 * 18.0 * s) as u32,
            |p| p[0] > 120 && p[1] > 120 && p[2] > 120,
        );
        assert_eq!(strip, 0, "scale {s}: text ends before the right margin");
    }
}

#[test]
fn a_long_cjk_line_wraps_by_display_width_and_draws_nothing_past_the_right_edge() {
    for s in SCALES2 {
        let h = window(s);
        // 300 wide characters: 600 columns, which one character per column would pack into rows twice too wide.
        let line: SharedString = "\u{65e5}\u{672c}\u{8a9e}".repeat(100).into();
        let lines = vec![line; 4];
        let (cols, n) = show_wrapped(&h, &lines);
        let (rows, _) = wrap_lines(&lines, cols, &|| false);
        assert_eq!(rows.len(), n);
        assert!(
            rows.iter().all(|r| r.chars().count() * 2 <= cols),
            "scale {s}: every row is at most {cols} columns wide"
        );
        let f = h.render();
        let y0 = (h.comp.get_panel_body_y() * s).round() as u32;
        let x0 = ((h.comp.get_panel_body_x() + h.comp.get_panel_gutter_w()) * s).round() as u32;
        let right = ((h.comp.get_panel_body_x() + h.comp.get_panel_body_w()) * s).round() as u32;
        let ink = f.count_where(x0, y0, right, y0 + (18.0 * s) as u32, |p| {
            p[0] > 90 && p[1] > 90 && p[2] > 90
        });
        assert!(ink > 20, "scale {s}: the first row is drawn ({ink})");
        let strip = f.count_where(
            right.saturating_sub((6.0 * s) as u32),
            y0,
            right.saturating_sub((4.0 * s) as u32),
            y0 + (6.0 * 18.0 * s) as u32,
            |p| p[0] > 120 && p[1] > 120 && p[2] > 120,
        );
        assert_eq!(strip, 0, "scale {s}: no wide glyph runs to the edge");
    }
}

#[test]
fn the_gutter_shows_a_number_on_the_first_row_of_a_line_only() {
    for s in SCALES2 {
        let h = window(s);
        h.comp.set_panel_open(true);
        h.comp.set_panel_kind(2);
        // Rows: line 1 (one row), line 2 (three rows), line 3 (one row).
        let rows: Vec<SharedString> = ["one", "two a", "two b", "two c", "three"]
            .iter()
            .map(|r| SharedString::from(*r))
            .collect();
        h.comp.set_panel_lines(model(rows));
        h.comp.set_panel_line_nums(model(vec![1, 2, 0, 0, 3]));
        h.comp.set_panel_wrapped(true);
        h.comp.set_panel_number_count(3);
        h.advance(0);
        let f = h.render();
        let row_h = (18.0 * s).round() as u32;
        let y0 = (h.comp.get_panel_body_y() * s).round() as u32;
        let x0 = (h.comp.get_panel_body_x() * s).round() as u32;
        let x1 = x0 + ((h.comp.get_panel_gutter_w() - 10.0) * s).round() as u32;
        let ink: Vec<usize> = (0..5u32)
            .map(|i| {
                f.count_where(x0, y0 + i * row_h, x1, y0 + (i + 1) * row_h, |p| {
                    p[0] > 60 && p[1] > 60 && p[2] > 60
                })
            })
            .collect();
        assert!(
            ink[0] > 3 && ink[1] > 3 && ink[4] > 3,
            "scale {s}: numbered rows show a number {ink:?}"
        );
        assert_eq!(
            ink[2], 0,
            "scale {s}: a continuation row shows none {ink:?}"
        );
        assert_eq!(
            ink[3], 0,
            "scale {s}: a continuation row shows none {ink:?}"
        );
    }
}

#[test]
fn unwrapped_rows_keep_counting_their_own_lines() {
    let h = window(1.0);
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(2);
    let rows: Vec<SharedString> = ["a", "b", "c"]
        .iter()
        .map(|r| SharedString::from(*r))
        .collect();
    h.comp.set_panel_lines(model(rows));
    h.comp.set_panel_line_nums(model(Vec::<i32>::new()));
    h.comp.set_panel_wrapped(false);
    h.advance(0);
    let f = h.render();
    let y0 = h.comp.get_panel_body_y() as u32;
    let x0 = h.comp.get_panel_body_x() as u32;
    let x1 = x0 + (h.comp.get_panel_gutter_w() - 10.0) as u32;
    for i in 0..3u32 {
        let ink = f.count_where(x0, y0 + i * 18, x1, y0 + (i + 1) * 18, |p| {
            p[0] > 60 && p[1] > 60 && p[2] > 60
        });
        assert!(ink > 3, "row {i} is numbered");
    }
}

#[test]
fn a_wrapped_view_has_no_horizontal_scroll_and_an_unwrapped_one_has() {
    let h = window(1.0);
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(2);
    h.comp.set_panel_max_cols(400);
    h.comp.set_panel_lines(model(vec![long_line()]));
    h.comp.set_panel_wrapped(false);
    h.advance(0);
    // Unwrapped: the Right key scrolls the line sideways (the scroll shows in the first visible column).
    let before = h.render();
    h.comp.invoke_focus_panel();
    h.advance(0);
    h.activate();
    h.key_text(&String::from(char::from(slint::platform::Key::RightArrow)));
    h.advance(0);
    let after = h.render();
    let (x0, y0) = (
        h.comp.get_panel_body_x() as u32,
        h.comp.get_panel_body_y() as u32,
    );
    let same = (0..20u32).all(|i| before.px(x0 + 60 + i, y0 + 9) == after.px(x0 + 60 + i, y0 + 9));
    assert!(!same, "an unwrapped line scrolls sideways");
    // Wrapped: the same key moves nothing.
    h.comp
        .set_panel_lines(model(vec![SharedString::from("short")]));
    h.comp.set_panel_wrapped(true);
    h.advance(0);
    let a = h.render();
    h.key_text(&String::from(char::from(slint::platform::Key::RightArrow)));
    h.advance(0);
    let b = h.render();
    let same = (0..200u32).all(|i| a.px(x0 + 40 + i, y0 + 9) == b.px(x0 + 40 + i, y0 + 9));
    assert!(same, "a wrapped view does not scroll sideways");
}

#[test]
fn wrapped_rows_stay_virtual_only_the_ones_in_view_are_built() {
    // 60 000 lines of five rows each: 300 000 rows. The window, not the file, decides how much is built; the proof is
    // time, as for the unwrapped file (building every row would not finish).
    for s in SCALES2 {
        let h = window(s);
        let mut rows = Vec::with_capacity(300_000);
        let mut nums = Vec::with_capacity(300_000);
        for i in 0..60_000i32 {
            for k in 0..5 {
                rows.push(SharedString::from(format!("row {k} of line {i}")));
                nums.push(if k == 0 { i + 1 } else { 0 });
            }
        }
        h.comp.set_panel_open(true);
        h.comp.set_panel_kind(2);
        h.comp.set_panel_number_count(60_000);
        h.comp.set_panel_lines(model(rows));
        h.comp.set_panel_line_nums(model(nums));
        h.comp.set_panel_wrapped(true);
        let t = Instant::now();
        h.advance(0);
        let f = h.render();
        assert!(t.elapsed().as_secs() < 20, "scale {s}: {:?}", t.elapsed());
        let ink = f.count_where(
            (h.comp.get_panel_body_x() * s) as u32,
            (h.comp.get_panel_body_y() * s) as u32,
            W * 2,
            H * 3,
            |p| p[0] > 90 && p[1] > 90 && p[2] > 90,
        );
        assert!(ink > 100, "scale {s}: the first rows are drawn ({ink})");
    }
}

#[test]
fn the_wrap_button_is_on_a_text_files_header_only_and_calls_back() {
    for s in SCALES2 {
        let h = window(s);
        let pressed = Rc::new(Cell::new(0u32));
        {
            let pressed = pressed.clone();
            h.comp
                .on_panel_wrap_toggled(move || pressed.set(pressed.get() + 1));
        }
        h.comp.set_panel_open(true);
        h.comp.set_panel_kind(2);
        h.comp.set_panel_lines(model(vec![SharedString::from("x")]));
        h.advance(0);
        let (x, y, w) = (
            h.comp.get_panel_wrap_button_x(),
            h.comp.get_panel_wrap_button_y(),
            h.comp.get_panel_wrap_button_w(),
        );
        assert!(w > 0.0, "scale {s}");
        // Inside the header, left of the close button.
        // The close button is 20 px wide, 8 px in from the slot's right edge.
        let close = h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w() - 28.0;
        assert!(
            x + w <= close + 0.5,
            "scale {s}: left of the close button ({x}+{w} vs {close})"
        );
        assert!(
            y >= h.comp.get_panel_slot_top() - 0.5
                && y < h.comp.get_panel_slot_top() + h.comp.get_panel_header_h()
        );
        h.click(x + w / 2.0, y + w / 2.0);
        assert_eq!(
            pressed.get(),
            1,
            "scale {s}: a click on the button calls back once"
        );
        // A picture's header has no such button: the click lands on nothing that toggles.
        h.comp.set_panel_kind(1);
        h.advance(0);
        h.click(x + w / 2.0, y + w / 2.0);
        assert_eq!(pressed.get(), 1, "scale {s}: only a text file wraps");
    }
}
