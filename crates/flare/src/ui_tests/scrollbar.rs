//! The terminal scrollbar's geometry at all eight display scales (parity M1 line 36): a 6 px
//! strip flush right, no track, a thumb with a 3 px radius at the ink's 40 percent, 65 percent
//! while the view is scrolled back, every edge on a whole device pixel.

use slint::ComponentHandle;

use crate::ScrollbarPreview;
use crate::Theme;
use crate::ui_harness::{Frame, Harness, SCALES};

fn bar(scale: f32) -> Harness<ScrollbarPreview> {
    let h = Harness::new(scale, 400, 300, || ScrollbarPreview::new().unwrap());
    h.advance(0);
    h
}

/// The ground blended toward the ink by `alpha`, the colour a thumb of that alpha shows.
fn blend(ground: [u8; 4], ink: [u8; 4], alpha: f32) -> [u8; 4] {
    let mix = |g: u8, i: u8| (f32::from(g) + (f32::from(i) - f32::from(g)) * alpha).round() as u8;
    [
        mix(ground[0], ink[0]),
        mix(ground[1], ink[1]),
        mix(ground[2], ink[2]),
        255,
    ]
}

fn ink(h: &Harness<ScrollbarPreview>) -> [u8; 4] {
    let c = h.comp.global::<Theme>().get_text();
    [c.red(), c.green(), c.blue(), 255]
}

#[test]
fn the_strip_is_six_px_flush_right_with_no_track_and_the_thumb_sits_where_asked() {
    for s in SCALES {
        let h = bar(s);
        let f = h.render();
        let ground = f.px(0, f.h / 2);
        let want_w = (6.0 * s).round() as u32;
        let (x0, y0, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, |p| !Frame::near(p, ground, 2))
            .unwrap_or_else(|| panic!("scale {s}: no thumb drawn"));
        assert_eq!(x1 + 1 - x0, want_w, "scale {s}: 6 css px wide");
        assert_eq!(x1 + 1, f.w, "scale {s}: flush with the right edge");
        // top 0.25 and len 0.5 of the track, each edge rounded to a device pixel
        let top = (0.25 * f.h as f32).round() as i64;
        let len = (0.5 * f.h as f32).round() as i64;
        assert!(
            (i64::from(y0) - top).abs() <= 1,
            "scale {s}: top {y0} vs {top}"
        );
        assert!(
            (i64::from(y1 + 1 - y0) - len).abs() <= 1,
            "scale {s}: length {} vs {len}",
            y1 + 1 - y0
        );
        // no track: the strip above and below the thumb is the bare ground
        for y in [2, y0.saturating_sub(3), y1 + 3, f.h - 3] {
            if y < f.h && (y < y0 || y > y1) {
                for x in (f.w - want_w)..f.w {
                    assert!(
                        Frame::near(f.px(x, y), ground, 2),
                        "scale {s}: track at ({x},{y})"
                    );
                }
            }
        }
        // no gutter: nothing but ground left of the strip
        assert_eq!(
            f.count_where(0, 0, f.w - want_w, f.h, |p| !Frame::near(p, ground, 2)),
            0,
            "scale {s}: nothing left of the strip"
        );
    }
}

#[test]
fn the_thumb_radius_is_three_css_px_at_every_scale() {
    for s in SCALES {
        let h = bar(s);
        let f = h.render();
        let ground = f.px(0, f.h / 2);
        let (x0, y0, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, |p| !Frame::near(p, ground, 2))
            .unwrap();
        let r = 3.0 * s;
        // the very corner pixel lies outside the circle: ground, while the edge middles are ink
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            assert!(
                Frame::near(f.px(x, y), ground, 6),
                "scale {s}: corner ({x},{y}) is rounded off"
            );
        }
        let mx = (x0 + x1) / 2;
        let my = (y0 + y1) / 2;
        assert!(
            !Frame::near(f.px(mx, y0 + 1), ground, 2),
            "scale {s}: top edge middle drawn"
        );
        assert!(
            !Frame::near(f.px(mx, my), ground, 2),
            "scale {s}: body drawn"
        );
        // a point inside the curve is fully ink: the radius is not larger than 3 px
        let d = (r * 0.30).ceil() as u32;
        let inside = f.px(x0 + d + 1, y0 + d + 1);
        assert!(
            !Frame::near(inside, ground, 2),
            "scale {s}: radius is not larger than 3 px"
        );
    }
}

#[test]
fn the_thumb_is_the_ink_at_40_percent_and_65_percent_when_scrolled_back_at_every_scale() {
    for s in SCALES {
        let h = bar(s);
        let f = h.render();
        let ground = f.px(0, f.h / 2);
        let ink = ink(&h);
        let (x, y) = (f.w - (3.0 * s).round() as u32, f.h / 2);
        assert!(
            Frame::near(f.px(x, y), blend(ground, ink, 0.40), 3),
            "scale {s}: live {:?} vs {:?}",
            f.px(x, y),
            blend(ground, ink, 0.40)
        );
        h.comp.set_back(true);
        h.advance(0);
        let f = h.render();
        assert!(
            Frame::near(f.px(x, y), blend(ground, ink, 0.65), 3),
            "scale {s}: back {:?} vs {:?}",
            f.px(x, y),
            blend(ground, ink, 0.65)
        );
    }
}

#[test]
fn nothing_is_drawn_when_there_is_no_history() {
    for s in SCALES {
        let h = bar(s);
        h.comp.set_on(false);
        h.advance(0);
        let f = h.render();
        let ground = f.px(0, f.h / 2);
        assert_eq!(
            f.count_where(0, 0, f.w, f.h, |p| !Frame::near(p, ground, 2)),
            0,
            "scale {s}: an idle shell draws no bar"
        );
    }
}
