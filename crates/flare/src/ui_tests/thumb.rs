//! #17: the hover thumbnail's bubble, on the headless harness at several scales.

use crate::ThumbBubblePreview;
use crate::ui_harness::{Frame, Harness};

const PIC: [u8; 4] = [10, 200, 30, 255];

fn picture(w: u32, h: u32) -> slint::Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    for p in buf.make_mut_slice() {
        *p = slint::Rgba8Pixel {
            r: PIC[0],
            g: PIC[1],
            b: PIC[2],
            a: PIC[3],
        };
    }
    slint::Image::from_rgba8(buf)
}

fn harness(scale: f32, w: u32, h: u32) -> Harness<ThumbBubblePreview> {
    let hn = Harness::new(scale, 600, 400, || {
        ThumbBubblePreview::new().expect("the preview builds")
    });
    hn.comp.set_picture(picture(w, h));
    hn.comp.set_pic_w(w as f32);
    hn.comp.set_pic_h(h as f32);
    hn.comp.set_at_x(40.0);
    hn.comp.set_at_y(30.0);
    hn.comp.set_caption("photo.png  1920 x 960".into());
    hn
}

fn up(hn: &Harness<ThumbBubblePreview>) {
    hn.comp.set_visible_thumb(true);
    hn.advance(300);
}

/// The device pixel at the centre of the picture, placed centred in the bubble with 8 px above it.
fn pic_centre(hn: &Harness<ThumbBubblePreview>) -> (u32, u32) {
    let x = 40.0 + hn.comp.get_bubble_w() / 2.0;
    let y = 30.0 + 8.0 + hn.comp.get_pic_h() / 2.0;
    (hn.phys(x), hn.phys(y))
}

#[test]
fn the_bubble_is_the_picture_plus_its_frame_at_every_scale() {
    for scale in [1.0, 1.25, 1.5, 2.25] {
        let hn = harness(scale, 256, 128);
        up(&hn);
        assert_eq!(hn.comp.get_bubble_w(), 256.0 + 16.0, "{scale}");
        assert_eq!(hn.comp.get_bubble_h(), 128.0 + 34.0, "{scale}");
        let f = hn.render();
        let (x, y) = pic_centre(&hn);
        assert!(Frame::near(f.px(x, y), PIC, 6), "{scale}: {:?}", f.px(x, y));
    }
}

#[test]
fn the_longest_side_is_256_css_px_on_the_screen_at_every_scale() {
    for scale in [1.0, 1.5, 2.25] {
        let hn = harness(scale, 256, 64);
        up(&hn);
        let f = hn.render();
        let (_, y) = pic_centre(&hn);
        let left = hn.phys(40.0 + 8.0 + 4.0);
        let run = f.run_len_h(y, left, |p| Frame::near(p, PIC, 6));
        // The run starts 4 px into the picture.
        let expect = hn.phys(256.0 - 4.0);
        assert!(run.abs_diff(expect) <= 2, "{scale}: {run} of {expect}");
    }
}

#[test]
fn a_tiny_picture_sits_centred_in_a_bubble_wide_enough_for_its_caption() {
    let hn = harness(1.0, 16, 16);
    up(&hn);
    assert_eq!(hn.comp.get_bubble_w(), 120.0 + 16.0);
    let f = hn.render();
    let (x, y) = pic_centre(&hn);
    assert!(Frame::near(f.px(x, y), PIC, 6), "{:?}", f.px(x, y));
}

#[test]
fn it_fades_in_and_draws_nothing_while_hidden() {
    let hn = harness(1.0, 64, 64);
    hn.advance(300);
    let (x, y) = pic_centre(&hn);
    let hidden = hn.render().px(x, y);
    assert!(!Frame::near(hidden, PIC, 40), "{hidden:?}");
    hn.comp.set_visible_thumb(true);
    hn.advance(20);
    let early = hn.render().px(x, y);
    assert!(!Frame::near(early, PIC, 6), "still fading: {early:?}");
    hn.advance(300);
    assert!(Frame::near(hn.render().px(x, y), PIC, 6));
}

#[test]
fn the_grid_under_the_bubble_keeps_getting_the_pointer() {
    let hn = harness(1.0, 64, 64);
    up(&hn);
    hn.render();
    let before = hn.comp.get_pointer_moves();
    hn.move_to(60.0, 50.0);
    hn.move_to(70.0, 55.0);
    assert!(hn.comp.get_pointer_moves() > before);
}
