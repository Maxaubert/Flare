//! M9 Task 24: the update chip and the notice under it, on the headless harness.
//!
//! Geometry comes from the preview's out properties (css pixels); colours are read from the rendered
//! frame. Task 33 appends the eight-scale tests.

use crate::UpdateChipPreview;
use crate::ui_harness::{Frame, Harness};

/// `Theme.sel-bg` (the chip fill), `Theme.on-accent` (the label ink), `Theme.side-flat` (the notice
/// surface), `Theme.text-soft` (the notice text), `Theme.tabs` (the title bar).
const SEL_BG: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const ON_ACCENT: [u8; 4] = [0x0b, 0x0b, 0x0f, 255];
const SIDE_FLAT: [u8; 4] = [0x1f, 0x1f, 0x1e, 255];
const TEXT_SOFT: [u8; 4] = [0xc2, 0xbf, 0xb5, 255];
const TABS: [u8; 4] = [0x17, 0x17, 0x17, 255];

const LABEL: &str = "Update 0.56.0";
const TIP: &str = "See what is new in 0.56.0";

fn harness(scale: f32) -> Harness<UpdateChipPreview> {
    let h = Harness::new(scale, 1000, 120, || {
        UpdateChipPreview::new().expect("the preview builds")
    });
    h.comp.set_shown(true);
    h.comp.set_label(LABEL.into());
    h.comp.set_aria("Update to 0.56.0".into());
    h.comp.set_tip(TIP.into());
    h.advance(0);
    h
}

/// The chip's box in device pixels: (x, y, w, h).
fn chip_px(h: &Harness<UpdateChipPreview>) -> (u32, u32, u32, u32) {
    let s = h.scale();
    (
        (h.comp.get_chip_x() * s).round() as u32,
        (h.comp.get_chip_y() * s).round() as u32,
        (h.comp.get_chip_w() * s).round() as u32,
        (h.comp.get_chip_h() * s).round() as u32,
    )
}

/// The chip's centre in css pixels (what the harness pointer takes).
fn chip_centre_css(h: &Harness<UpdateChipPreview>) -> (f32, f32) {
    (
        h.comp.get_chip_x() + h.comp.get_chip_w() / 2.0,
        h.comp.get_chip_y() + h.comp.get_chip_h() / 2.0,
    )
}

/// A device pixel inside the chip's left padding, clear of the corner radius and the icon.
fn fill_probe(h: &Harness<UpdateChipPreview>) -> (u32, u32) {
    let (x, y, _, hh) = chip_px(h);
    let s = h.scale();
    (x + (8.0 * s).round() as u32, y + hh / 2)
}

/// Distance between two colours, summed over the three colour channels.
fn dist(a: [u8; 4], b: [u8; 4]) -> u32 {
    (0..3).map(|i| u32::from(a[i].abs_diff(b[i]))).sum()
}

#[test]
fn nothing_is_drawn_without_an_offer() {
    let h = harness(1.0);
    h.comp.set_shown(false);
    h.advance(0);
    let f = h.render();
    let (x, y, w, hh) = chip_px(&h);
    for yy in y..y + hh {
        for xx in x..x + w {
            assert!(
                Frame::near(f.px(xx, yy), TABS, 1),
                "pixel {xx},{yy} is {:?}",
                f.px(xx, yy)
            );
        }
    }
}

#[test]
fn the_chip_is_24_css_px_high_at_100_150_and_225() {
    for s in [1.0_f32, 1.5, 2.25] {
        let h = harness(s);
        let f = h.render();
        let (x, _, _, _) = chip_px(&h);
        let col = x + (8.0 * s).round() as u32;
        let bb = f
            .bbox_where(col, 0, col + 1, f.h, |p| Frame::near(p, SEL_BG, 4))
            .expect("the fill is drawn");
        let rows = bb.3 - bb.1 + 1;
        assert_eq!(rows, (24.0 * s).round() as u32, "scale {s}");
    }
}

#[test]
fn the_fill_is_sel_bg_and_not_grey() {
    let h = harness(1.0);
    let f = h.render();
    let (px, py) = fill_probe(&h);
    let p = f.px(px, py);
    assert!(Frame::near(p, SEL_BG, 1), "{p:?}");
    let spread = p[..3].iter().max().unwrap() - p[..3].iter().min().unwrap();
    assert!(spread >= 40, "channel spread {spread}");
}

#[test]
fn the_label_is_on_accent_and_clears_4_5_to_1() {
    let h = harness(1.0);
    let f = h.render();
    let (x, y, w, hh) = chip_px(&h);
    let label_x = h.comp.get_label_x() as u32;
    // The ink is the label pixel furthest from the fill.
    let mut ink = SEL_BG;
    for yy in y..y + hh {
        for xx in (x + label_x)..(x + w - 10) {
            let p = f.px(xx, yy);
            if dist(p, SEL_BG) > dist(ink, SEL_BG) {
                ink = p;
            }
        }
    }
    assert!(dist(ink, ON_ACCENT) <= 24, "ink {ink:?}");
    let ratio = flare_theme::ansi::contrast_ratio("#0b0b0f", "#fe8f34");
    assert!(ratio >= 4.5, "contrast {ratio}");
}

#[test]
fn the_icon_is_an_11_px_box_centred_vertically() {
    let h = harness(1.5);
    let icon_h = h.comp.get_icon_h();
    assert!((icon_h - 11.0).abs() < 1e-3, "icon box {icon_h}");
    // The icon's centre sits on the chip's centre to within one device pixel.
    let centre = h.comp.get_icon_y() + icon_h / 2.0;
    let chip_mid = h.comp.get_chip_h() / 2.0;
    assert!(
        (centre - chip_mid).abs() <= 1.0 / h.scale(),
        "centre {centre}"
    );
}

#[test]
fn padding_and_gap_are_10_and_6() {
    for s in [1.0_f32, 1.5, 2.25] {
        let h = harness(s);
        let icon_x = h.comp.get_icon_x();
        let label_x = h.comp.get_label_x();
        assert!(
            (icon_x * s - (10.0 * s).round()).abs() < 1e-3,
            "icon {icon_x}"
        );
        assert!(
            (label_x * s - (27.0 * s).round()).abs() < 1e-3,
            "label {label_x}"
        );
    }
}

#[test]
fn hover_brightens_the_fill_by_ten_percent() {
    let h = harness(1.0);
    let before = h.render();
    let (px, py) = fill_probe(&h);
    let b = before.px(px, py);
    let (x, y) = chip_centre_css(&h);
    h.move_to(x, y);
    h.advance(0);
    let after = h.render().px(px, py);
    let delta: Vec<i32> = (0..3)
        .map(|i| i32::from(after[i]) - i32::from(b[i]))
        .collect();
    assert!(delta.iter().all(|d| *d >= 0), "{delta:?}");
    assert!(delta.iter().any(|d| *d >= 6), "{delta:?}");
    assert!(delta.iter().all(|d| *d <= 16), "{delta:?}");
}

#[test]
fn a_click_emits_clicked_once_and_no_tip() {
    let h = harness(1.0);
    let (x, y) = chip_centre_css(&h);
    h.move_to(x, y);
    h.advance(0);
    let tips = h.comp.get_tip_requests();
    h.click(x, y);
    h.advance(0);
    assert_eq!(h.comp.get_clicks(), 1);
    assert_eq!(
        h.comp.get_tip_requests(),
        tips,
        "the click asked for no tip"
    );
    assert_eq!(h.comp.get_dismissals(), 0);
}

#[test]
fn the_width_is_the_same_in_every_phase() {
    let h = harness(1.0);
    let idle = h.comp.get_chip_w();
    h.comp.set_busy(true);
    h.advance(0);
    assert_eq!(h.comp.get_chip_w(), idle);
    h.comp.set_busy(false);
}

#[test]
fn a_tip_is_requested_once_on_hover() {
    let h = harness(1.0);
    let (x, y) = chip_centre_css(&h);
    h.move_to(x, y);
    h.move_to(x + 1.0, y);
    assert_eq!(h.comp.get_tip_requests(), 1);
    assert_eq!(h.comp.get_last_tip().as_str(), TIP);
}

#[test]
fn the_notice_has_a_flat_surface_a_hairline_and_the_text() {
    let h = harness(1.0);
    h.comp
        .set_notice("Preview only: nothing was installed".into());
    h.advance(0);
    let f = h.render();
    let nx = h.comp.get_notice_x() as u32;
    let ny = h.comp.get_notice_y() as u32;
    let nw = h.comp.get_notice_w() as u32;
    let nh = h.comp.get_notice_h() as u32;
    // Inside the hairline the surface is the opaque side-flat.
    assert!(Frame::near(f.px(nx + nw / 2, ny + 2), SIDE_FLAT, 2));
    // The text is text-soft somewhere in the box.
    let text = f.bbox_where(nx + 10, ny + 4, nx + nw - 10, ny + nh - 4, |p| {
        Frame::near(p, TEXT_SOFT, 12)
    });
    assert!(text.is_some(), "no text-soft pixel in the notice");
}

#[test]
fn a_click_on_the_notice_dismisses_it() {
    let h = harness(1.0);
    h.comp
        .set_notice("The update could not be downloaded.".into());
    h.advance(0);
    let nx = h.comp.get_notice_x();
    let ny = h.comp.get_notice_y();
    let nw = h.comp.get_notice_w();
    let nh = h.comp.get_notice_h();
    h.click(nx + nw / 2.0, ny + nh / 2.0);
    assert_eq!(h.comp.get_dismissals(), 1);
    assert_eq!(h.comp.get_clicks(), 0);
}

#[test]
fn the_notice_is_hidden_with_empty_text() {
    let h = harness(1.0);
    h.comp.set_notice("".into());
    h.advance(0);
    let f = h.render();
    let nx = h.comp.get_notice_x() as u32;
    let ny = h.comp.get_notice_y() as u32;
    let nw = h.comp.get_notice_w() as u32;
    let nh = h.comp.get_notice_h() as u32;
    assert!(Frame::near(f.px(nx + nw / 2, ny + nh / 2), TABS, 1));
}
