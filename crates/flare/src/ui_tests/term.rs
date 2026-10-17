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

// find bar
// The find bar (Task 17): 332 x 28 plus two hairlines, 12 px from the top right, no shadow.

use crate::FindBarPreview;

/// `Theme.side-flat`, and `Theme.hover` blended over it.
const SIDE_FLAT: [u8; 4] = [31, 31, 30, 255];
const HOVER_OVER_SIDE: [u8; 4] = [44, 44, 43, 255];
const BORDER_OVER_SIDE: [u8; 4] = [46, 46, 45, 255];

fn find(scale: f32) -> Harness<FindBarPreview> {
    let h = Harness::new(scale, 800, 400, || FindBarPreview::new().unwrap());
    h.comp.set_open(true);
    h.advance(0);
    h
}

fn not_ground(p: [u8; 4]) -> bool {
    !Frame::near(p, GROUND, 2)
}

fn bar_box(f: &Frame, s: f32) -> (u32, u32, u32, u32) {
    f.bbox_where(0, 0, f.w, f.h, not_ground)
        .unwrap_or_else(|| panic!("scale {s}: no bar drawn"))
}

fn hair_of(s: f32) -> u32 {
    s.floor().max(1.0) as u32
}

#[test]
fn the_bar_sits_12_px_from_the_top_right_at_every_scale() {
    for s in SCALES {
        let h = find(s);
        let f = h.render();
        let (_, y0, x1, _) = bar_box(&f, s);
        let right_gap = f.w - (x1 + 1);
        let want = (12.0 * s).round() as u32;
        assert!(
            right_gap.abs_diff(want) <= 1,
            "scale {s}: right gap {right_gap}, want {want}"
        );
        assert!(
            y0.abs_diff(want) <= 1,
            "scale {s}: top gap {y0}, want {want}"
        );
    }
}

#[test]
fn the_bar_is_332_by_28_plus_two_hairlines() {
    for s in SCALES {
        let h = find(s);
        let f = h.render();
        let (x0, y0, x1, y1) = bar_box(&f, s);
        let hair = hair_of(s);
        let w = x1 + 1 - x0;
        let ht = y1 + 1 - y0;
        let want_w = (332.0 * s).round() as u32 + 2 * hair;
        let want_h = (28.0 * s).round() as u32 + 2 * hair;
        assert!(
            w.abs_diff(want_w) <= 1,
            "scale {s}: width {w}, want {want_w}"
        );
        assert!(
            ht.abs_diff(want_h) <= 1,
            "scale {s}: height {ht}, want {want_h}"
        );
    }
}

#[test]
fn the_border_is_a_hairline_and_the_fill_is_side_flat() {
    for s in [1.0_f32, 1.5, 2.25] {
        let h = find(s);
        let f = h.render();
        let (x0, y0, _, y1) = bar_box(&f, s);
        let mid = (y0 + y1) / 2;
        let hair = hair_of(s);
        let border = f.run_len_h(mid, x0, |p| Frame::near(p, BORDER_OVER_SIDE, 4));
        assert!(
            border.abs_diff(hair) <= 1,
            "scale {s}: border run {border}, want {hair}"
        );
        let interior = f.px(x0 + hair + 2, mid);
        assert!(
            Frame::near(interior, SIDE_FLAT, 2),
            "scale {s}: interior {interior:?}"
        );
    }
}

#[test]
fn there_is_no_shadow() {
    for s in [1.0_f32, 2.0] {
        let h = find(s);
        let f = h.render();
        let (x0, y0, x1, y1) = bar_box(&f, s);
        let mid_y = (y0 + y1) / 2;
        let mid_x = (x0 + x1) / 2;
        for d in [1u32, 3, 6] {
            let samples = [
                f.px(x0 - d, mid_y),
                f.px(mid_x, y1 + d),
                f.px(mid_x, y0 - d),
            ];
            for p in samples {
                assert!(Frame::near(p, GROUND, 1), "scale {s}, {d} px out: {p:?}");
            }
        }
    }
}

#[test]
fn typing_reports_the_query_and_enter_steps() {
    let h = find(1.0);
    h.comp.invoke_focus_input();
    h.key_text("abc");
    assert_eq!(h.comp.get_last_query().as_str(), "abc");
    h.key_text("\n");
    assert_eq!(h.comp.get_next_count(), 1);
    h.key_with(true, false, "\n");
    assert_eq!(h.comp.get_previous_count(), 1);
    assert_eq!(h.comp.get_next_count(), 1);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_close_count(), 1);
    assert_eq!(h.comp.get_query().as_str(), "abc");
}

#[test]
fn the_placeholder_is_drawn_only_while_empty() {
    let h = find(1.0);
    let f = h.render();
    let (x0, y0, _, y1) = bar_box(&f, 1.0);
    let in_x0 = x0 + 1 + 8;
    let in_x1 = in_x0 + 180;
    let ink = |p: [u8; 4]| !Frame::near(p, SIDE_FLAT, 8);
    let empty = f
        .bbox_where(in_x0, y0 + 2, in_x1, y1, ink)
        .expect("the placeholder is drawn while empty");
    assert!(empty.2 - empty.0 > 40, "placeholder spans {empty:?}");
    h.comp.set_query("x".into());
    h.advance(0);
    let f = h.render();
    let typed = f.bbox_where(in_x0, y0 + 2, in_x1, y1, ink);
    let width = typed.map_or(0, |b| b.2 - b.0);
    assert!(width < 20, "only the typed letter remains: {typed:?}");
}

#[test]
fn opening_focuses_the_input() {
    let h = find(1.0);
    h.comp.set_open(false);
    h.advance(0);
    h.comp.set_open(true);
    h.advance(0);
    h.key_text("z");
    assert_eq!(h.comp.get_query().as_str(), "z");
}

#[test]
fn the_readout_is_right_aligned_with_a_52_px_minimum() {
    for s in [1.0_f32, 2.0] {
        let h = find(s);
        h.comp.set_readout("3 of 12".into());
        h.advance(0);
        let f = h.render();
        let (x0, y0, _, y1) = bar_box(&f, s);
        let hair = hair_of(s);
        let box_x0 = x0 + hair + ((8.0 + 180.0 + 6.0) * s).round() as u32;
        let box_x1 = box_x0 + (52.0 * s).round() as u32;
        let ink = |p: [u8; 4]| !Frame::near(p, SIDE_FLAT, 8);
        let b = f
            .bbox_where(box_x0, y0 + hair, box_x1, y1 + 1 - hair, ink)
            .unwrap_or_else(|| panic!("scale {s}: no readout ink"));
        let gap = (box_x1 - 1).abs_diff(b.2);
        assert!(
            gap <= s.ceil() as u32 + 1,
            "scale {s}: ink right {} box right {}",
            b.2,
            box_x1 - 1
        );
        h.comp.set_readout("".into());
        h.advance(0);
        let f2 = h.render();
        assert!(
            f2.bbox_where(box_x0, y0 + hair, box_x1, y1 + 1 - hair, ink)
                .is_none(),
            "scale {s}: ink with no readout"
        );
        let (a, _, c, _) = bar_box(&f2, s);
        let want = (332.0 * s).round() as u32 + 2 * hair;
        assert!((c + 1 - a).abs_diff(want) <= 1, "scale {s}: width changed");
    }
}

#[test]
fn previous_and_next_do_nothing_without_hits() {
    let h = find(1.0);
    h.comp.set_can_step(false);
    h.advance(0);
    h.click(718.0, 26.0);
    h.click(744.0, 26.0);
    assert_eq!(
        (h.comp.get_previous_count(), h.comp.get_next_count()),
        (0, 0)
    );
    h.click(770.0, 26.0);
    assert_eq!(h.comp.get_close_count(), 1);
    h.comp.set_can_step(true);
    h.advance(0);
    h.click(718.0, 26.0);
    h.click(744.0, 26.0);
    assert_eq!(
        (h.comp.get_previous_count(), h.comp.get_next_count()),
        (1, 1)
    );
}

#[test]
fn a_hovered_button_lights_a_20_px_square() {
    for s in SCALES {
        let h = find(s);
        h.comp.set_can_step(true);
        h.advance(0);
        h.move_to(744.0, 26.0);
        h.advance(0);
        let f = h.render();
        let (bx0, by0, bx1, by1) = bar_box(&f, s);
        let hair = hair_of(s);
        let b = f
            .bbox_where(
                h.phys(744.0 - 14.0).max(bx0 + hair),
                by0 + hair,
                h.phys(744.0 + 14.0).min(bx1 + 1 - hair),
                by1 + 1 - hair,
                |p| Frame::near(p, HOVER_OVER_SIDE, 2),
            )
            .unwrap_or_else(|| panic!("scale {s}: no hover fill"));
        let want = (20.0 * s).round() as u32;
        assert!(
            (b.2 + 1 - b.0).abs_diff(want) <= 1,
            "scale {s}: width {}",
            b.2 + 1 - b.0
        );
        assert!(
            (b.3 + 1 - b.1).abs_diff(want) <= 1,
            "scale {s}: height {}",
            b.3 + 1 - b.1
        );
    }
}

#[test]
fn no_hex_colour_literal_in_find_slint() {
    let text = include_str!("../../ui/find.slint");
    for (i, _) in text.match_indices('#') {
        let run: String = text[i + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        let lower = run.to_ascii_lowercase();
        assert!(
            run.is_empty() || lower.starts_with("ffffff") || lower.starts_with("000000"),
            "find.slint holds the colour #{run}"
        );
    }
}
