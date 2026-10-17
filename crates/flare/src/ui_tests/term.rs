//! Component and flow tests of the Milestone 5 terminal (Tasks 17 to 25); headless only.

// badge
// The Copied badge (Task 18): drawn at the bottom centre, 24 px up, at every display scaling.

use crate::BadgePreview;
use crate::ui_harness::{Frame, Harness, SCALES};

/// `Theme.ground`, `Theme.control` and `Theme.divider` over the ground (the hairline blends).
const GROUND: [u8; 4] = [23, 23, 23, 255];
const CONTROL: [u8; 4] = [30, 30, 29, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];

fn badge(scale: f32) -> Harness<BadgePreview> {
    Harness::new(scale, 400, 300, || BadgePreview::new().unwrap())
}

fn is_ink(p: [u8; 4]) -> bool {
    !Frame::near(p, GROUND, 2)
}

#[test]
fn the_badge_is_not_drawn_while_gone() {
    let h = badge(1.0);
    h.comp.set_phase(0);
    h.advance(0);
    let f = h.render();
    assert_eq!(f.count_where(0, 0, f.w, f.h, is_ink), 0);
}

#[test]
fn the_pill_is_centred_and_24_px_from_the_bottom_at_every_scale() {
    for s in SCALES {
        let h = badge(s);
        h.comp.set_phase(1);
        h.advance(500);
        let f = h.render();
        let (x0, _, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_ink)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let centre = (x0 + x1 + 1) as f32 / 2.0;
        assert!(
            (centre - f.w as f32 / 2.0).abs() <= 1.0,
            "scale {s}: centre {centre} of width {}",
            f.w
        );
        let bottom = y1 + 1;
        let want = f.h - (24.0 * s).round() as u32;
        assert!(
            bottom.abs_diff(want) <= 1,
            "scale {s}: bottom {bottom}, want {want}"
        );
    }
}

#[test]
fn the_pill_has_a_hairline_border_and_a_fill() {
    for s in [1.0_f32, 2.25] {
        let h = badge(s);
        h.comp.set_phase(1);
        h.advance(500);
        let f = h.render();
        let (x0, y0, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_ink)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let mid = (y0 + y1) / 2;
        let hair = (s.floor().max(1.0)) as u32;
        let border = f.run_len_h(mid, x0, |p| Frame::near(p, DIVIDER, 4));
        assert!(
            border.abs_diff(hair) <= 1,
            "scale {s}: border run {border}, want {hair}"
        );
        let interior = f.px(x0 + hair + 3, mid);
        assert!(
            Frame::near(interior, CONTROL, 2),
            "scale {s}: interior {interior:?}"
        );
        assert!(x1 > x0 && y1 > y0);
    }
}

#[test]
fn leaving_fades_out() {
    let h = badge(1.0);
    h.comp.set_phase(1);
    h.advance(500);
    h.comp.set_phase(2);
    h.advance(200);
    let f = h.render();
    assert_eq!(f.count_where(0, 0, f.w, f.h, is_ink), 0);
}
