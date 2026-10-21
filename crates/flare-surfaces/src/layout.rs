//! The panel slot's layout maths, in physical pixels.
//!
//! The slot sits at the right of the page (the part of the window right of the sidebar). Its width
//! is a share of the page, kept per tab, clamped so the terminal always keeps room and the panel
//! is never too narrow to read. The `.slint` file evaluates the same formulas in logical pixels
//! (`Px.snap`); `ui_tests` pins the two to each other at 100, 150, 225 and 300 percent.

/// The slot never gets narrower than this (logical px), unless the page cannot give it so much.
pub const MIN_SLOT: f64 = 240.0;
/// The terminal keeps at least this much (logical px).
pub const MIN_GRID: f64 = 320.0;
/// The slot never takes more than this share of the page.
pub const MAX_SHARE: f64 = 0.7;
/// Below this width (logical px) the slot cannot be drawn usefully and is not shown.
pub const USABLE_MIN: f64 = 120.0;
/// The share a tab's slot has until the user drags the splitter.
pub const DEFAULT_SHARE: f32 = 0.4;
/// The splitter's hit area (logical px), centred on the slot's left edge.
pub const SPLITTER_HIT: f64 = 8.0;
/// The panel header's height (logical px).
pub const HEADER_H: f64 = 32.0;

/// Where the slot is, in physical px of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotLayout {
    /// The slot's width; 0 when it is not shown.
    pub slot_w: i32,
    /// The terminal's width (the page less the slot).
    pub grid_w: i32,
    /// False when the page is too narrow for a slot.
    pub shown: bool,
    /// The narrowest and widest the slot may be now (the splitter's limits).
    pub min_w: i32,
    pub max_w: i32,
}

fn px(logical: f64, scale: f64) -> i32 {
    (logical * scale).round() as i32
}

fn sane(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The slot's limits for a page `page_w` wide: (min, max, shown).
pub fn limits(page_w: i32, scale: f64) -> (i32, i32, bool) {
    let scale = sane(scale);
    let page_w = page_w.max(0);
    let by_share = (f64::from(page_w) * MAX_SHARE).floor() as i32;
    let by_grid = page_w - px(MIN_GRID, scale);
    let max_w = by_share.min(by_grid).max(0);
    let min_w = px(MIN_SLOT, scale).min(max_w);
    (min_w, max_w, max_w >= px(USABLE_MIN, scale))
}

/// The slot for a page `page_w` wide at `scale`, with the tab's `share` (None: the default).
pub fn slot_layout(page_w: i32, scale: f64, share: Option<f32>) -> SlotLayout {
    let page_w = page_w.max(0);
    let (min_w, max_w, shown) = limits(page_w, scale);
    if !shown {
        return SlotLayout {
            slot_w: 0,
            grid_w: page_w,
            shown: false,
            min_w,
            max_w,
        };
    }
    let share = share
        .filter(|s| s.is_finite() && *s > 0.0 && *s < 1.0)
        .unwrap_or(DEFAULT_SHARE);
    let want = (f64::from(page_w) * f64::from(share)).round() as i32;
    let slot_w = want.clamp(min_w, max_w);
    SlotLayout {
        slot_w,
        grid_w: page_w - slot_w,
        shown: true,
        min_w,
        max_w,
    }
}

/// The slot width the splitter asks for when the pointer is at `pointer_x` (physical px from the
/// page's left edge): the page's right edge minus the pointer, kept inside the limits.
pub fn drag_to(page_w: i32, scale: f64, pointer_x: i32) -> i32 {
    let (min_w, max_w, _) = limits(page_w, scale);
    (page_w - pointer_x).clamp(min_w, max_w)
}

/// The share to store for a slot `slot_w` wide on a page `page_w` wide, or None when the page has
/// no width.
pub fn share_of(slot_w: i32, page_w: i32) -> Option<f32> {
    (page_w > 0).then(|| (slot_w as f32 / page_w as f32).clamp(0.01, 0.99))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 4] = [1.0, 1.5, 2.25, 3.0];

    #[test]
    fn the_default_share_is_two_fifths_inside_the_limits() {
        let l = slot_layout(1000, 1.0, None);
        assert_eq!((l.slot_w, l.grid_w, l.shown), (400, 600, true));
        assert_eq!((l.min_w, l.max_w), (240, 680));
    }

    #[test]
    fn the_slot_and_the_terminal_add_up_to_the_page_at_every_scale() {
        for scale in SCALES {
            for logical_w in [600.0, 840.0, 1000.0, 1400.0, 1920.0, 3000.0] {
                let page = px(logical_w, scale);
                for share in [None, Some(0.1), Some(0.4), Some(0.9)] {
                    let l = slot_layout(page, scale, share);
                    assert_eq!(l.slot_w + l.grid_w, page, "{scale} {logical_w} {share:?}");
                    if l.shown {
                        assert!(l.slot_w >= l.min_w && l.slot_w <= l.max_w);
                        assert!(l.grid_w >= px(MIN_GRID, scale), "{scale} {logical_w}");
                    } else {
                        assert_eq!(l.slot_w, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn the_limits_scale_with_the_display() {
        for scale in SCALES {
            let page = px(1600.0, scale);
            let (min_w, max_w, shown) = limits(page, scale);
            assert!(shown);
            assert_eq!(min_w, px(MIN_SLOT, scale), "{scale}");
            assert_eq!(max_w, (f64::from(page) * 0.7).floor() as i32, "{scale}");
        }
    }

    #[test]
    fn a_page_too_narrow_for_a_slot_shows_none_and_the_terminal_keeps_all() {
        for scale in SCALES {
            let page = px(400.0, scale);
            let l = slot_layout(page, scale, Some(0.5));
            assert!(!l.shown, "{scale}");
            assert_eq!((l.slot_w, l.grid_w), (0, page));
        }
    }

    #[test]
    fn a_page_between_shows_a_slot_narrower_than_the_minimum() {
        // 500 logical px: the terminal keeps 320, so the slot has 180, under the 240 minimum.
        let l = slot_layout(500, 1.0, None);
        assert!(l.shown);
        assert_eq!((l.slot_w, l.min_w, l.max_w), (180, 180, 180));
    }

    #[test]
    fn a_bad_scale_reads_as_one() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(slot_layout(1000, bad, None), slot_layout(1000, 1.0, None));
        }
    }

    #[test]
    fn a_bad_share_reads_as_the_default() {
        for bad in [0.0f32, 1.0, -1.0, 7.0, f32::NAN] {
            assert_eq!(
                slot_layout(1000, 1.0, Some(bad)),
                slot_layout(1000, 1.0, None)
            );
        }
    }

    #[test]
    fn dragging_clamps_to_the_limits_at_every_scale() {
        for scale in SCALES {
            let page = px(1500.0, scale);
            let (min_w, max_w, _) = limits(page, scale);
            // Pointer past the window's right edge: the narrowest slot.
            assert_eq!(drag_to(page, scale, page + 500), min_w);
            // Pointer at the page's left edge: the widest.
            assert_eq!(drag_to(page, scale, 0), max_w);
            assert_eq!(drag_to(page, scale, -300), max_w);
            // In between it follows the pointer.
            let x = page - (min_w + max_w) / 2;
            assert_eq!(drag_to(page, scale, x), (min_w + max_w) / 2);
        }
    }

    #[test]
    fn a_dragged_width_stores_as_a_share_that_lays_out_the_same() {
        for scale in SCALES {
            let page = px(1500.0, scale);
            let slot = drag_to(page, scale, page - px(500.0, scale));
            let share = share_of(slot, page).unwrap();
            let l = slot_layout(page, scale, Some(share));
            assert!(
                (l.slot_w - slot).abs() <= 1,
                "{scale}: {} vs {slot}",
                l.slot_w
            );
        }
        assert_eq!(share_of(10, 0), None);
        assert_eq!(share_of(0, 100), Some(0.01));
        assert_eq!(share_of(100, 100), Some(0.99));
    }

    #[test]
    fn a_negative_page_is_an_empty_one() {
        let l = slot_layout(-5, 1.0, None);
        assert_eq!((l.slot_w, l.grid_w, l.shown), (0, 0, false));
    }
}
