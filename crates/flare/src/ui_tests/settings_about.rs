//! UI tests of the About card and the Slint badge (M7 Task 38); headless only.
//!
//! Every test runs on the headless harness (no window). Box geometry comes from the preview's out
//! properties (logical px); colours and glyph ink are read from the rendered frame. The badge's
//! press goes to a callback the preview records, never to `Platform.open-url`, so no browser opens.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::AboutPreview;
use crate::Theme;
use crate::ui_harness::{Frame, Harness, SCALES};

/// The dark pt-default tokens: `Theme.text`, `Theme.dim`, `Theme.ground`.
const INK: [u8; 4] = [0xde, 0xda, 0xcf, 255];
const DIM: [u8; 4] = [0x92, 0x90, 0x89, 255];
const GROUND_DARK: [u8; 4] = [23, 23, 23, 255];

/// A light ground with dark ink, for the light-theme badge test.
const GROUND_LIGHT: [u8; 4] = [0xf5, 0xf4, 0xef, 255];
const TEXT_LIGHT: [u8; 4] = [0x1d, 0x1c, 0x19, 255];
const DIM_LIGHT: [u8; 4] = [0x5c, 0x5a, 0x55, 255];

const CSS_W: u32 = 480;
const CSS_H: u32 = 260;

/// The link the badge opens (AboutSlint's own address).
const SLINT_URL: &str = "https://slint.dev";

fn harness(scale: f32, light: bool) -> Harness<AboutPreview> {
    Harness::new(scale, CSS_W, CSS_H, move || {
        let p = AboutPreview::new().unwrap();
        if light {
            // Set before the first show, so the badge's `init` reads the light scheme.
            let t = p.global::<Theme>();
            t.set_light(true);
            t.set_ground(GROUND_LIGHT.into_slint());
            t.set_text(TEXT_LIGHT.into_slint());
            t.set_dim(DIM_LIGHT.into_slint());
        }
        p
    })
}

/// Converts the test's RGBA triple into a Slint colour.
trait IntoSlint {
    fn into_slint(self) -> slint::Color;
}

impl IntoSlint for [u8; 4] {
    fn into_slint(self) -> slint::Color {
        slint::Color::from_argb_u8(self[3], self[0], self[1], self[2])
    }
}

/// WCAG relative luminance of an opaque colour.
fn rel_lum(p: [u8; 4]) -> f64 {
    let lin = |c: u8| {
        let s = f64::from(c) / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])
}

/// WCAG contrast ratio between two opaque colours.
fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (la, lb) = (rel_lum(a), rel_lum(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The box of the badge in logical px: (x, y, w, h).
fn badge_box(h: &Harness<AboutPreview>) -> (f32, f32, f32, f32) {
    (
        h.comp.get_badge_x(),
        h.comp.get_badge_y(),
        h.comp.get_badge_w(),
        h.comp.get_badge_h(),
    )
}

/// The device-pixel box of a logical box, rounded outwards.
fn device_box(
    h: &Harness<AboutPreview>,
    (x, y, w, hgt): (f32, f32, f32, f32),
) -> (u32, u32, u32, u32) {
    let s = h.scale();
    let x0 = (x * s).floor() as u32;
    let y0 = (y * s).floor() as u32;
    let x1 = ((x + w) * s).ceil() as u32;
    let y1 = ((y + hgt) * s).ceil() as u32;
    (x0, y0, x1, y1)
}

#[test]
fn the_card_is_96_high_with_a_56_icon_at_every_scale() {
    for scale in SCALES {
        let h = harness(scale, false);
        assert!(
            (h.comp.get_card_h() - 96.0).abs() < 0.01,
            "the card is 96 logical px high at scale {scale}, got {}",
            h.comp.get_card_h()
        );
        assert!(
            (h.comp.get_icon_w() - 56.0).abs() < 0.01,
            "the app mark is 56 logical px at scale {scale}, got {}",
            h.comp.get_icon_w()
        );
    }
}

#[test]
fn the_heading_is_18_px_bold_and_the_lines_are_dim() {
    let h = harness(1.0, false);
    let f = h.render();
    // The text column starts at 18 + 56 + 18 = 92 logical px, inside the 96 px card.
    let (x0, y0, x1, y1) = (92, 0, CSS_W, 96);
    let near = |p: [u8; 4], c: [u8; 4]| Frame::near(p, c, 6);

    let heading = f
        .bbox_where(x0, y0, x1, y1, |p| near(p, INK))
        .expect("the heading is drawn in Theme.text");
    let (_, hy0, _, hy1) = heading;
    let heading_h = hy1 - hy0 + 1;
    // An 18 px bold capital run is about 13 px tall at 1x; a 12.5 px line is shorter.
    assert!(
        (12..=19).contains(&heading_h),
        "the heading ink is {heading_h} px tall, expected an 18 px bold line"
    );

    // The two lines below the heading are drawn in Theme.dim, under it.
    let dim = f
        .bbox_where(x0, hy1 + 1, x1, y1, |p| near(p, DIM))
        .expect("the lines below the heading are drawn in Theme.dim");
    assert!(
        dim.1 > hy1,
        "the dim lines sit below the heading (dim top {}, heading bottom {hy1})",
        dim.1
    );
}

#[test]
fn the_badge_is_legible_on_a_dark_and_a_light_theme() {
    for (light, fill_ground) in [(false, GROUND_DARK), (true, GROUND_LIGHT)] {
        let h = harness(1.0, light);
        let f = h.render();
        let (bx, by, bw, bh) = badge_box(&h);
        assert!(bh > 40.0, "the badge is a real block, {bh} px high");
        let (x0, y0, x1, y1) = device_box(&h, (bx, by, bw, bh));

        // The panel fill: sampled just inside the badge's left edge, clear of the logo.
        let fill = f.px(x0 + 2, y0 + 2);
        assert!(
            Frame::near(fill, fill_ground, 12),
            "the panel is near the ground colour (got {fill:?}, light={light})"
        );

        // The glyph pixel furthest from the panel fill (the logo and the 10 px version line both
        // draw in it) must read at 4.5:1 against the fill.
        let mut best: Option<([u8; 4], f64)> = None;
        for y in y0..y1.min(f.h) {
            for x in x0..x1.min(f.w) {
                let p = f.px(x, y);
                let d = (rel_lum(p) - rel_lum(fill)).abs();
                if best.is_none_or(|(_, bd)| d > bd) {
                    best = Some((p, d));
                }
            }
        }
        let (ink, _) = best.expect("the badge has pixels");
        let ratio = contrast(ink, fill);
        assert!(
            ratio >= 4.5,
            "the badge ink {ink:?} reads {ratio:.2}:1 on {fill:?} (light={light}), needs 4.5"
        );
    }
}

#[test]
fn the_badge_opens_slint_dev_on_a_press() {
    let h = harness(1.0, false);
    let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = opened.clone();
    h.comp
        .on_open_url(move |u| sink.borrow_mut().push(u.to_string()));

    let (bx, by, bw, bh) = badge_box(&h);
    h.click(bx + bw / 2.0, by + bh / 2.0);

    assert_eq!(
        opened.borrow().as_slice(),
        [SLINT_URL.to_string()],
        "one press opens https://slint.dev once"
    );
}

#[test]
fn badge_h_is_stable_across_scales() {
    let heights: Vec<f32> = SCALES
        .iter()
        .map(|&s| harness(s, false).comp.get_badge_h())
        .collect();
    let first = heights[0];
    assert!(first > 40.0, "the badge has a real height, {first} px");
    for (s, h) in SCALES.iter().zip(&heights) {
        assert!(
            (h - first).abs() < 0.01,
            "the badge is {h} logical px at scale {s}, but {first} at 1x"
        );
    }
}

/// The badge draws no box (its background is transparent), so only its top and left edges, where
/// the logo starts, are held to whole device pixels. Its height stays AboutSlint's own
/// measure, so it is the same logical number at every scale.
#[test]
fn the_badge_origin_sits_on_whole_device_pixels_at_every_scale() {
    for scale in SCALES {
        let h = harness(scale, false);
        let (bx, by, _, _) = badge_box(&h);
        for v in [bx, by] {
            let dev = v * scale;
            assert!(
                (dev - dev.round()).abs() < 0.01,
                "a badge origin at {v} logical px is {dev} device px at scale {scale}"
            );
        }
    }
}
