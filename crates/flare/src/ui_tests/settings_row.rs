//! UI tests of the row, section and panel frames (M7 Task 34); headless only.
//!
//! Geometry of `RowPreview` in css px: the panel sits at (20, 20) and is `panel_w` wide; row 1 is
//! y 20..78, row 2 y 78..136, row 3 y 136..194. Every size is a multiple of a quarter, so every
//! edge lands on a whole device pixel at all eight scalings.

use slint::{Color, ComponentHandle};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{RowPreview, Theme};

const LONG_SUB: &str = "Pick the folder that new tabs open in.";

fn harness(scale: f32, panel_w: f32) -> Harness<RowPreview> {
    let h = Harness::new(scale, 640, 400, || RowPreview::new().unwrap());
    h.comp.set_panel_w(panel_w);
    h.advance(0);
    h
}

/// A css length as device pixels at `scale`.
fn dev(scale: f32, css: f32) -> u32 {
    (css * scale).round() as u32
}

fn rgb(c: Color) -> [u8; 4] {
    [c.red(), c.green(), c.blue(), 255]
}

#[test]
fn the_tile_is_32_square_and_the_label_starts_at_60() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        assert!(
            (h.comp.get_label_x() - 60.0).abs() < 0.01,
            "scale {scale}: label-x {}",
            h.comp.get_label_x()
        );
        // The tile fill is the colour of the tile's top middle, clear of the icon; its bbox is 32 by 32.
        let f = h.render();
        let fill = f.px(dev(scale, 50.0), dev(scale, 36.0));
        let bb = f
            .bbox_where(
                dev(scale, 30.0),
                dev(scale, 25.0),
                dev(scale, 70.0),
                dev(scale, 78.0),
                |p| Frame::near(p, fill, 1),
            )
            .expect("the tile is drawn");
        assert_eq!(bb.0, dev(scale, 34.0), "scale {scale}: left edge");
        assert_eq!(bb.1, dev(scale, 33.0), "scale {scale}: top edge");
        assert_eq!(bb.2 - bb.0 + 1, dev(scale, 32.0), "scale {scale}: width");
        assert_eq!(bb.3 - bb.1 + 1, dev(scale, 32.0), "scale {scale}: height");
    }
}

#[test]
fn the_rule_starts_after_the_icon_column_and_after_a_block_it_starts_at_the_edge() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        // Row 2's top rule is on device row 78 * scale; 3 css px lower is inside the row, clear of text.
        let y_rule = dev(scale, 78.0);
        let y_in = dev(scale, 81.0);
        let first_rule_x = |f: &Frame| -> Option<u32> {
            (dev(scale, 21.0)..f.w).find(|&x| f.px(x, y_rule) != f.px(x, y_in))
        };
        let x0 = first_rule_x(&h.render()).expect("row 2 has a rule");
        assert_eq!(
            x0,
            dev(scale, 80.0),
            "scale {scale}: rule after the icon column"
        );

        h.comp.set_after_block(true);
        h.advance(0);
        let x0 = first_rule_x(&h.render()).expect("row 2 has a rule");
        assert!(
            x0 <= dev(scale, 21.0),
            "scale {scale}: a rule after a block starts at the edge, got {x0}"
        );
    }
}

#[test]
fn the_first_row_has_no_rule() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        assert!(!h.comp.get_first_rule(), "scale {scale}: row 1 is first");
        assert!(h.comp.get_second_rule(), "scale {scale}: row 2 has a rule");
    }
}

#[test]
fn a_tap_row_fires_tapped_off_the_control_only() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        // The label area of row 1 (css 100, 49).
        h.click(100.0, 49.0);
        assert_eq!(h.comp.get_taps(), 1, "scale {scale}: label press");
        assert_eq!(h.comp.get_control_taps(), 0, "scale {scale}");
        // The control: 36 wide, its right edge 16 px in from the row's right edge (css 564).
        assert!(
            (h.comp.get_control_right() - 544.0).abs() < 0.01,
            "scale {scale}"
        );
        h.click(546.0, 49.0);
        assert_eq!(h.comp.get_control_taps(), 1, "scale {scale}: control press");
        assert_eq!(
            h.comp.get_taps(),
            1,
            "scale {scale}: the control press is not a row tap"
        );
    }
}

#[test]
fn an_off_row_is_dimmed_and_ignores_the_pointer() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        h.click(100.0, 165.0);
        assert_eq!(
            h.comp.get_off_taps(),
            0,
            "scale {scale}: an off row ignores taps"
        );
        // Dimmed: the tile's top-middle pixel of row 3 is darker than row 1's.
        let f = h.render();
        let on = f.px(dev(scale, 50.0), dev(scale, 36.0));
        let off = f.px(dev(scale, 50.0), dev(scale, 152.0));
        assert!(
            on[0] > off[0] + 2,
            "scale {scale}: off tile {off:?} is not dimmed against {on:?}"
        );
    }
}

#[test]
fn the_subtext_tooltip_appears_only_when_it_is_cut() {
    for scale in SCALES {
        // A 260 px panel: row 2's long sub is cut, row 1's short sub fits.
        let h = harness(scale, 260.0);
        h.move_to(100.0, 49.0);
        assert_eq!(
            h.comp.get_tip_count(),
            0,
            "scale {scale}: a short sub has no tip"
        );
        h.move_to(100.0, 114.0);
        assert!(
            h.comp.get_tip_count() > 0,
            "scale {scale}: a cut sub fires tip"
        );
        assert_eq!(h.comp.get_tip_text().as_str(), LONG_SUB, "scale {scale}");
    }
}

#[test]
fn the_flash_fills_the_row() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        let (x, y) = (dev(scale, 500.0), dev(scale, 25.0));
        let base = h.render().px(x, y);
        h.comp.set_flash(1.0);
        h.advance(0);
        let lit = h.render().px(x, y);
        // SettingsInk.flash is accent-hi at 16 percent, painted over the panel.
        let accent = rgb(h.comp.global::<Theme>().get_accent_hi());
        let a = 0.16f32;
        for c in 0..3 {
            let want = base[c] as f32 * (1.0 - a) + accent[c] as f32 * a;
            assert!(
                (lit[c] as f32 - want).abs() <= 3.0,
                "scale {scale} channel {c}: {lit:?} is not the flash over {base:?}"
            );
        }
    }
}

#[test]
fn the_panel_clips_its_corners() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        h.comp.set_flash(1.0);
        h.advance(0);
        let corner = h.render().px(dev(scale, 20.0), dev(scale, 20.0));
        // The panel's outer corner lies outside its 5 px radius: the window ground shows.
        let ground = rgb(h.comp.global::<Theme>().get_ground());
        assert!(
            Frame::near(corner, ground, 3),
            "scale {scale}: corner {corner:?} is painted by the flash, ground {ground:?}"
        );
    }
}

#[test]
fn no_accent_in_the_frames_but_the_flash() {
    for scale in SCALES {
        let h = harness(scale, 560.0);
        let f = h.render();
        let warm = f.count_where(0, 0, f.w, f.h, |p| p[0] as i32 - p[2] as i32 > 60);
        assert_eq!(warm, 0, "scale {scale}: accent-like pixels without a flash");
    }
}
