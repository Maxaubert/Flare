//! Pixel and geometry tests of the dictation pill and the dictation tab mark at all eight scalings (M8 Task 23).
//!
//! Parity lines 872, 881 to 883 (PT `DictationPill.tsx`, `DictationTabMark.tsx`). The pill is absolutely placed over
//! the terminal and never takes layout, so the grid's pixels do not move when it appears.

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{DictationPillPreview, DictationTabMarkPreview};
use slint::Model;

/// `Theme.ground` (the window behind the grid).
const GROUND: [u8; 4] = [23, 23, 23, 255];
/// `Theme.control`: the tab in front of the paste target.
const CONTROL: [u8; 4] = [30, 30, 29, 255];
/// `Theme.side-flat`: the pill's opaque fill.
const SIDE_FLAT: [u8; 4] = [31, 31, 30, 255];
/// `Theme.accent-hi`: the bars, the spinner and the tab mark.
const ACCENT_HI: [u8; 4] = [246, 162, 91, 255];
/// `Theme.dim`: the live text.
const DIM: [u8; 4] = [146, 144, 137, 255];
/// The preview's grid rows (the stripes the pill must not move).
const STRIPE: [u8; 4] = [94, 92, 89, 255];

/// PT `BARS`.
const BARS: [f32; 5] = [0.55, 0.85, 1.0, 0.8, 0.5];

fn pill(scale: f32) -> Harness<DictationPillPreview> {
    Harness::new(scale, 400, 300, || DictationPillPreview::new().unwrap())
}

fn is_fill(p: [u8; 4]) -> bool {
    Frame::near(p, SIDE_FLAT, 2)
}

#[test]
fn nothing_is_mounted_while_idle() {
    for s in SCALES {
        let h = pill(s);
        h.comp.set_phase(0);
        h.advance(0);
        let f = h.render();
        assert_eq!(
            f.count_where(0, 0, f.w, f.h, is_fill),
            0,
            "scale {s}: the pill is drawn while idle"
        );
    }
}

#[test]
fn the_pill_is_bottom_centre_16px_up() {
    for s in SCALES {
        let h = pill(s);
        h.comp.set_phase(1);
        h.advance(500);
        let f = h.render();
        let (x0, _, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_fill)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let centre = (x0 + x1 + 1) as f32 / 2.0;
        assert!(
            (centre - f.w as f32 / 2.0).abs() <= 1.0,
            "scale {s}: centre {centre} of width {}",
            f.w
        );
        let want = f.h - (16.0 * s).round() as u32;
        assert!(
            (y1 + 1).abs_diff(want) <= 2,
            "scale {s}: bottom {}, want {want}",
            y1 + 1
        );
    }
}

/// The first row of the pill's band (its shadow included), in device pixels: rows above it are the grid.
fn band_top(f: &Frame, s: f32) -> u32 {
    f.h - (80.0 * s).round() as u32
}

#[test]
fn the_pill_never_moves_the_grid() {
    for s in SCALES {
        let h = pill(s);
        h.comp.set_phase(0);
        h.advance(0);
        let without = h.render();
        assert!(
            without.count_where(0, 0, without.w, without.h, |p| Frame::near(p, STRIPE, 2)) > 0,
            "scale {s}: the grid rows are not drawn"
        );
        h.comp.set_phase(1);
        h.comp
            .set_live_text("the newest words of a long sentence".into());
        h.advance(500);
        let with = h.render();
        assert_eq!(with.w, without.w);
        // Every pixel above the pill's band is the same with and without it.
        let band = band_top(&with, s);
        for y in 0..band {
            for x in 0..with.w {
                assert_eq!(
                    with.px(x, y),
                    without.px(x, y),
                    "scale {s}: the pill moved the grid at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn bars_follow_the_level_rule() {
    let h = pill(1.0);
    h.comp.set_phase(1);
    let want = |shown: f32| -> Vec<f32> {
        BARS.iter()
            .map(|b| (shown * b * 1.25).clamp(0.12, 1.0))
            .collect()
    };
    // Rising is immediate: the shown level is the level.
    h.comp.set_level(0.5);
    h.advance(70);
    assert!(
        (h.comp.get_shown() - 0.5).abs() < 1e-4,
        "shown {}",
        h.comp.get_shown()
    );
    let got: Vec<f32> = h.comp.get_bars().iter().collect();
    for (g, w) in got.iter().zip(want(0.5)) {
        assert!((g - w).abs() < 1e-3, "bars {got:?}, want {:?}", want(0.5));
    }
    // Falling is smoothed: shown * 0.82 + level * 0.18.
    h.comp.set_level(0.0);
    h.advance(70);
    assert!(
        (h.comp.get_shown() - 0.41).abs() < 1e-3,
        "shown {}",
        h.comp.get_shown()
    );
    let got: Vec<f32> = h.comp.get_bars().iter().collect();
    for (g, w) in got.iter().zip(want(0.41)) {
        assert!((g - w).abs() < 1e-3, "bars {got:?} after falling");
    }
    // A silent meter sinks to the floor of every bar.
    for i in 0..15 {
        h.comp.set_level(if i % 2 == 0 { 0.0 } else { 0.00001 });
        h.advance(70);
    }
    h.advance(70);
    assert!(h.comp.get_bars().iter().all(|b| (b - 0.12).abs() < 1e-3));
}

#[test]
fn live_text_shows_the_newest_words() {
    for s in [1.0_f32, 2.25] {
        let h = pill(s);
        h.comp.set_phase(1);
        h.comp.set_live_text(
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen"
                .into(),
        );
        h.advance(500);
        let f = h.render();
        let (_, y0, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_fill)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let mut dim_right = None;
        for y in y0..=y1 {
            for x in 0..f.w {
                if Frame::near(f.px(x, y), DIM, 40) {
                    dim_right = dim_right.max(Some(x));
                }
            }
        }
        let dim_right = dim_right.unwrap_or_else(|| panic!("scale {s}: no live text drawn"));
        // The last word ends where the right padding (16 px) begins.
        let pad = (16.0 * s).round() as u32;
        // The glyph's side bearing and the antialiased edge sit inside a few device pixels.
        let tol = (6.0 * s).round() as u32;
        assert!(
            dim_right + pad + tol >= x1 && dim_right + pad <= x1 + 2,
            "scale {s}: live text ends at {dim_right}, pill right edge {x1}"
        );
    }
}

fn mark(scale: f32) -> Harness<DictationTabMarkPreview> {
    Harness::new(scale, 400, 36, || DictationTabMarkPreview::new().unwrap())
}

#[test]
fn the_mark_sits_on_the_target_tab_with_another_in_front() {
    for s in SCALES {
        let h = mark(s);
        h.comp.set_active(true);
        h.advance(0);
        let f = h.render();
        let target_end = (250.0 * s).round() as u32;
        // The target tab is the window's ground; the tab in front is Theme.control. Any other ink is the mark.
        let on_target = f.count_where(0, 0, target_end, f.h, |p| !Frame::near(p, GROUND, 6));
        let on_front = f.count_where(target_end, 0, f.w, f.h, |p| !Frame::near(p, CONTROL, 6));
        assert!(on_target > 0, "scale {s}: no mark on the target tab");
        assert_eq!(
            on_front, 0,
            "scale {s}: the mark spilled onto the tab in front"
        );
        assert!(
            f.count_where(0, 0, f.w, f.h, |p| Frame::near(p, ACCENT_HI, 60)) > 0,
            "scale {s}: the mark is not accent-hi"
        );
    }
}

#[test]
fn the_tab_mark_is_not_drawn_while_idle() {
    for s in SCALES {
        let h = mark(s);
        h.comp.set_active(false);
        h.advance(0);
        let f = h.render();
        assert_eq!(
            f.count_where(0, 0, f.w, f.h, |p| Frame::near(p, ACCENT_HI, 30)),
            0,
            "scale {s}: the mark is drawn while idle"
        );
    }
}

#[test]
fn the_pill_is_mounted_in_the_window_over_the_terminal_area() {
    use crate::AppWindow;
    for s in SCALES {
        let h = Harness::new(s, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        h.comp.set_dict_phase(0);
        h.advance(0);
        let idle = h.render();
        let left = h.phys(h.comp.get_grid_left());
        let top = h.phys(h.comp.get_grid_top());
        let below = top + h.phys(300.0);
        assert_eq!(
            idle.count_where(left, below, idle.w, idle.h, is_fill),
            0,
            "scale {s}: a pill is drawn while idle"
        );
        h.comp.set_dict_phase(1);
        h.advance(500);
        let on = h.render();
        let (x0, _, x1, y1) = on
            .bbox_where(left, below, on.w, on.h, is_fill)
            .unwrap_or_else(|| panic!("scale {s}: the window draws no pill"));
        let centre = (x0 + x1 + 1) as f32 / 2.0;
        let area_mid = (left + on.w) as f32 / 2.0;
        assert!(
            (centre - area_mid).abs() <= 2.0,
            "scale {s}: centre {centre}, area middle {area_mid}"
        );
        let want = on.h - (16.0 * s).round() as u32;
        assert!(
            (y1 + 1).abs_diff(want) <= 2,
            "scale {s}: bottom {}, want {want}",
            y1 + 1
        );
    }
}
