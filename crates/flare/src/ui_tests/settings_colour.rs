//! UI tests of the colour field and the colour popover (M7 Task 36); headless only.
//!
//! The preview is a 520 by 440 window with a field at (20, 20) and the popover overlay. Geometry comes
//! from the preview's out properties (logical px, device-snapped); colours are read from the frame.

use std::cell::RefCell;
use std::rc::Rc;

use slint::Color;
use slint::platform::{Key, PointerEventButton};

use crate::ColourPreview;
use crate::ui_harness::{Frame, Harness, SCALES};

const CSS_W: u32 = 520;
const CSS_H: u32 = 440;

fn make(scale: f32) -> Harness<ColourPreview> {
    let h = Harness::new(scale, CSS_W, CSS_H, || ColourPreview::new().unwrap());
    h.advance(0);
    h
}

/// A harness with the popover open (hue 0, so the hue colour is red), settled.
fn open_popover(scale: f32) -> Harness<ColourPreview> {
    let h = make(scale);
    h.comp.set_hue_colour(Color::from_rgb_u8(255, 0, 0));
    h.comp.set_open(true);
    h.advance(0);
    h
}

fn key(k: Key) -> String {
    char::from(k).to_string()
}

fn within(a: f32, b: f32, s: f32) -> bool {
    (a * s - b * s).abs() <= 1.0
}

/// The device pixel under a logical point.
fn at(f: &Frame, s: f32, x: f32, y: f32) -> [u8; 4] {
    f.px((x * s).round() as u32, (y * s).round() as u32)
}

type KeyLog = Rc<RefCell<Vec<(i32, String, bool)>>>;

fn log_keys(h: &Harness<ColourPreview>) -> KeyLog {
    let log: KeyLog = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    h.comp
        .on_slider_key(move |w, k, s| l.borrow_mut().push((w, k.to_string(), s)));
    log
}

fn count_calls(register: impl FnOnce(Box<dyn Fn()>)) -> Rc<RefCell<u32>> {
    let n = Rc::new(RefCell::new(0u32));
    let c = n.clone();
    register(Box::new(move || *c.borrow_mut() += 1));
    n
}

#[test]
fn field_widths_76_and_172_and_swatch_36_by_24() {
    for s in SCALES {
        let h = make(s);
        for (wide, want) in [(false, 76.0), (true, 172.0)] {
            h.comp.set_field_wide(wide);
            h.advance(0);
            let c = &h.comp;
            assert!(within(c.get_code_w(), want, s), "scale {s}: code {want}");
            assert!(within(c.get_swatch_w(), 36.0, s), "scale {s}: swatch w");
            assert!(within(c.get_swatch_h(), 24.0, s), "scale {s}: swatch h");
            assert!(
                within(c.get_swatch_x() - c.get_code_w(), 6.0, s),
                "scale {s}: the gap is 6"
            );
            // every edge sits on a whole device pixel
            for e in [
                c.get_code_w(),
                c.get_swatch_x(),
                c.get_swatch_x() + c.get_swatch_w(),
            ] {
                let d = e * s;
                assert!(
                    (d - d.round()).abs() < 1e-2,
                    "scale {s}: edge {e} is off a pixel"
                );
            }
        }
    }
}

#[test]
fn popover_is_226_wide_the_square_200_by_140_the_bars_12_high() {
    for s in SCALES {
        let h = open_popover(s);
        let c = &h.comp;
        assert!(within(c.get_pop_w(), 226.0, s), "scale {s}: popover width");
        assert!(within(c.get_sv_w(), 200.0, s), "scale {s}: square w");
        assert!(within(c.get_sv_h(), 140.0, s), "scale {s}: square h");
        assert!(within(c.get_hue_w(), 200.0, s), "scale {s}: hue w");
        assert!(within(c.get_hue_h(), 12.0, s), "scale {s}: hue h");
        assert!(within(c.get_alpha_w(), 200.0, s), "scale {s}: alpha w");
        assert!(within(c.get_alpha_h(), 12.0, s), "scale {s}: alpha h");
        // 12 between the square and the hue bar, and between the hue bar and the alpha bar
        assert!(
            within(c.get_hue_y() - (c.get_sv_y() + c.get_sv_h()), 12.0, s),
            "scale {s}: gap under the square"
        );
        assert!(
            within(c.get_alpha_y() - (c.get_hue_y() + c.get_hue_h()), 12.0, s),
            "scale {s}: gap under the hue bar"
        );
        // 12 padding plus the border: the square starts 13 in
        assert!(
            within(c.get_sv_x() - 200.0, 13.0, s),
            "scale {s}: left inset"
        );
        // the footer: 28 high, 12 below the last bar
        assert!(within(c.get_foot_h(), 28.0, s), "scale {s}: footer h");
        assert!(
            within(
                c.get_foot_y() - (c.get_alpha_y() + c.get_alpha_h()),
                12.0,
                s
            ),
            "scale {s}: footer gap"
        );
        assert!(
            within(c.get_pop_h(), 254.0, s),
            "scale {s}: height with alpha"
        );
        assert!(within(c.get_fmt_w(), 54.0, s), "scale {s}: format button");
        assert!(within(c.get_drop_w(), 28.0, s), "scale {s}: eyedropper");
        // without an alpha the popover is a bar and 12 shorter
        h.comp.set_alpha_on(false);
        h.advance(0);
        assert!(
            within(h.comp.get_pop_h(), 230.0, s),
            "scale {s}: height without alpha"
        );
    }
}

#[test]
fn the_three_sliders_have_prism_terminals_names_in_order() {
    let h = open_popover(1.0);
    let c = &h.comp;
    assert_eq!(c.get_sv_name(), "Saturation and brightness");
    assert_eq!(c.get_hue_name(), "Hue");
    assert_eq!(c.get_alpha_name(), "Alpha");
    assert!(c.get_sv_y() < c.get_hue_y() && c.get_hue_y() < c.get_alpha_y());
}

#[test]
fn the_square_paints_white_to_hue_and_to_black() {
    for s in SCALES {
        let h = open_popover(s);
        let f = h.render();
        let c = &h.comp;
        let (x0, y0, w, hh) = (c.get_sv_x(), c.get_sv_y(), c.get_sv_w(), c.get_sv_h());
        let inset = 4.0;
        let tl = at(&f, s, x0 + inset, y0 + inset);
        let tr = at(&f, s, x0 + w - inset, y0 + inset);
        let bl = at(&f, s, x0 + inset, y0 + hh - inset);
        let br = at(&f, s, x0 + w - inset, y0 + hh - inset);
        // top-left white, top-right the pure hue (red), both bottom corners black
        assert!(
            Frame::near(tl, [255, 255, 255, 255], 26),
            "scale {s}: top-left {tl:?}"
        );
        assert!(
            Frame::near(tr, [255, 0, 0, 255], 26),
            "scale {s}: top-right {tr:?}"
        );
        assert!(
            Frame::near(bl, [0, 0, 0, 255], 26),
            "scale {s}: bottom-left {bl:?}"
        );
        assert!(
            Frame::near(br, [0, 0, 0, 255], 26),
            "scale {s}: bottom-right {br:?}"
        );
    }
}

/// The six-stop spectrum at a fraction of the bar, as the CSS gradient reads.
fn spectrum(t: f32) -> [u8; 4] {
    let stops: [(f32, [f32; 3]); 7] = [
        (0.0, [255.0, 0.0, 0.0]),
        (1.0 / 6.0, [255.0, 255.0, 0.0]),
        (2.0 / 6.0, [0.0, 255.0, 0.0]),
        (3.0 / 6.0, [0.0, 255.0, 255.0]),
        (4.0 / 6.0, [0.0, 0.0, 255.0]),
        (5.0 / 6.0, [255.0, 0.0, 255.0]),
        (1.0, [255.0, 0.0, 0.0]),
    ];
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.0 && t <= b.0 {
            let k = (t - a.0) / (b.0 - a.0);
            let m = |i: usize| (a.1[i] + (b.1[i] - a.1[i]) * k).round() as u8;
            return [m(0), m(1), m(2), 255];
        }
    }
    [255, 0, 0, 255]
}

#[test]
fn the_hue_bar_paints_the_spectrum() {
    for s in SCALES {
        let h = open_popover(s);
        // the thumb rides the far end, out of the way of every sample
        h.comp.set_hue(359.0);
        h.advance(0);
        let f = h.render();
        let c = &h.comp;
        let (x0, y0, w, hh) = (c.get_hue_x(), c.get_hue_y(), c.get_hue_w(), c.get_hue_h());
        for t in [0.0f32, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 5.0 / 6.0] {
            // the first sample sits 6 px in: the bar's end is a half circle
            let dx = if t == 0.0 { 6.0 } else { t * w };
            let got = at(&f, s, x0 + dx, y0 + hh / 2.0);
            let want = spectrum(dx / w);
            assert!(
                Frame::near(got, want, 20),
                "scale {s}, t {t}: got {got:?} want {want:?}"
            );
        }
    }
}

#[test]
fn the_chip_splits_solid_and_see_through_over_a_checkerboard() {
    for s in SCALES {
        let h = make(s);
        // red at 50 percent
        h.comp.set_field_colour(Color::from_argb_u8(128, 255, 0, 0));
        h.advance(0);
        let f = h.render();
        let c = &h.comp;
        let (sx, sy) = (c.get_fld_x() + c.get_swatch_x(), c.get_fld_y());
        let mut left_bad = 0;
        let (mut light, mut dark) = (0, 0);
        for iy in 3..21 {
            for ix in 3..16 {
                let p = at(&f, s, sx + ix as f32, sy + iy as f32);
                if !Frame::near(p, [255, 0, 0, 255], 2) {
                    left_bad += 1;
                }
            }
            for ix in 20..33 {
                let p = at(&f, s, sx + ix as f32, sy + iy as f32);
                if Frame::near(p, [249, 122, 122, 255], 6) {
                    light += 1;
                } else if Frame::near(p, [225, 98, 98, 255], 6) {
                    dark += 1;
                }
            }
        }
        assert_eq!(left_bad, 0, "scale {s}: the left half is solid red");
        assert!(
            light > 20,
            "scale {s}: red over the light squares ({light})"
        );
        assert!(dark > 20, "scale {s}: red over the dark squares ({dark})");
    }
}

#[test]
fn a_press_on_the_square_reports_positions() {
    let h = open_popover(1.0);
    let log = Rc::new(RefCell::new(Vec::<(f32, f32, bool)>::new()));
    let l = log.clone();
    h.comp
        .on_sv_moved(move |x, y, k| l.borrow_mut().push((x, y, k)));
    let c = &h.comp;
    let (x0, y0, w, hh) = (c.get_sv_x(), c.get_sv_y(), c.get_sv_w(), c.get_sv_h());
    // the four corners, one pixel in, then the middle
    for (px, py, ex, ey) in [
        (x0 + 1.0, y0 + 1.0, 0.0, 0.0),
        (x0 + w - 1.0, y0 + 1.0, 1.0, 0.0),
        (x0 + 1.0, y0 + hh - 1.0, 0.0, 1.0),
        (x0 + w - 1.0, y0 + hh - 1.0, 1.0, 1.0),
        (x0 + w / 2.0, y0 + hh / 2.0, 0.5, 0.5),
    ] {
        log.borrow_mut().clear();
        h.move_to(px, py);
        h.press(px, py, PointerEventButton::Left);
        h.release(px, py, PointerEventButton::Left);
        let got = log.borrow().clone();
        assert_eq!(got.len(), 1, "one report per press");
        assert!(
            (got[0].0 - ex).abs() < 0.02 && (got[0].1 - ey).abs() < 0.02,
            "{got:?} want {ex},{ey}"
        );
        assert!(!got[0].2, "a pointer is not a key");
    }
    // a drag reports every move, and never leaves 0..1
    log.borrow_mut().clear();
    h.move_to(x0 + 10.0, y0 + 10.0);
    h.press(x0 + 10.0, y0 + 10.0, PointerEventButton::Left);
    h.move_to(x0 + w + 80.0, y0 + hh + 80.0);
    h.release(x0 + w + 80.0, y0 + hh + 80.0, PointerEventButton::Left);
    let last = *log.borrow().last().unwrap();
    assert!(
        (last.0 - 1.0).abs() < 1e-3 && (last.1 - 1.0).abs() < 1e-3,
        "clamped: {last:?}"
    );
    // the hue and alpha bars report their x
    let hue = Rc::new(RefCell::new(Vec::<f32>::new()));
    let alpha = Rc::new(RefCell::new(Vec::<f32>::new()));
    let (hl, al) = (hue.clone(), alpha.clone());
    h.comp.on_hue_moved(move |x, _| hl.borrow_mut().push(x));
    h.comp.on_alpha_moved(move |x, _| al.borrow_mut().push(x));
    let (hx, hy, hw, hh2) = (c.get_hue_x(), c.get_hue_y(), c.get_hue_w(), c.get_hue_h());
    h.click(hx + hw / 4.0, hy + hh2 / 2.0);
    assert!((hue.borrow()[0] - 0.25).abs() < 0.02, "{:?}", hue.borrow());
    let (ax, ay, aw, ah) = (
        c.get_alpha_x(),
        c.get_alpha_y(),
        c.get_alpha_w(),
        c.get_alpha_h(),
    );
    h.click(ax + aw * 0.75, ay + ah / 2.0);
    assert!(
        (alpha.borrow()[0] - 0.75).abs() < 0.02,
        "{:?}",
        alpha.borrow()
    );
}

#[test]
fn arrow_keys_and_shift_arrows_fire_slider_key() {
    let h = open_popover(1.0);
    let log = log_keys(&h);
    h.comp.invoke_focus_popover();
    h.key_text(&key(Key::LeftArrow));
    h.key_with(true, false, &key(Key::RightArrow));
    h.key_text(&key(Key::UpArrow));
    h.key_with(true, false, &key(Key::DownArrow));
    assert_eq!(
        *log.borrow(),
        vec![
            (0, "Left".to_string(), false),
            (0, "Right".to_string(), true),
            (0, "Up".to_string(), false),
            (0, "Down".to_string(), true),
        ]
    );
    // Tab to the hue bar: arrows, Home and End are its
    log.borrow_mut().clear();
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Home));
    h.key_with(true, false, &key(Key::LeftArrow));
    h.key_text(&key(Key::End));
    assert_eq!(
        *log.borrow(),
        vec![
            (1, "Home".to_string(), false),
            (1, "Left".to_string(), true),
            (1, "End".to_string(), false),
        ]
    );
    // and the alpha bar next
    log.borrow_mut().clear();
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::RightArrow));
    assert_eq!(*log.borrow(), vec![(2, "Right".to_string(), false)]);
}

#[test]
fn tab_and_shift_tab_stay_inside_the_popover() {
    let h = open_popover(1.0);
    let log = log_keys(&h);
    let formats = count_calls(|f| h.comp.on_format_clicked(f));
    let dropper = count_calls(|f| h.comp.on_dropper_clicked(f));
    h.comp.invoke_focus_popover();
    // forward: square, hue, alpha, eyedropper, format, then round to the square
    for _ in 0..3 {
        h.key_text(&key(Key::Tab));
    }
    h.key_text(" ");
    assert_eq!(*dropper.borrow(), 1, "the third Tab reaches the eyedropper");
    h.key_text(&key(Key::Tab));
    h.key_text(" ");
    assert_eq!(
        *formats.borrow(),
        1,
        "the fourth Tab reaches the format button"
    );
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::LeftArrow));
    assert_eq!(
        log.borrow().last().unwrap().0,
        0,
        "the fifth Tab is back on the square"
    );
    // backwards from the square goes to the last button
    h.key_with(true, false, &key(Key::Tab));
    h.key_text(" ");
    assert_eq!(
        *formats.borrow(),
        2,
        "Shift+Tab from the square reaches the format button"
    );
    // with no eyedropper and no alpha the order is square, hue, format
    h.comp.set_dropper_on(false);
    h.comp.set_alpha_on(false);
    h.advance(0);
    h.comp.invoke_focus_popover();
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Tab));
    h.key_text(" ");
    assert_eq!(*formats.borrow(), 3);
    assert_eq!(*dropper.borrow(), 1, "no eyedropper, no press");
}

#[test]
fn escape_is_consumed_by_the_popover_and_by_a_field_with_a_draft() {
    let h = open_popover(1.0);
    let escapes = count_calls(|f| h.comp.on_escape_pressed(f));
    h.comp.invoke_focus_popover();
    h.key_text(&key(Key::Escape));
    assert_eq!(*escapes.borrow(), 1, "the popover answers Escape");
    // from a button inside it as well
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Escape));
    assert_eq!(*escapes.borrow(), 2);

    // the field: with a draft Escape is its own; without one it is left to whoever's it was
    let f = make(1.0);
    let esc = count_calls(|g| f.comp.on_field_escape(g));
    let edits = Rc::new(RefCell::new(Vec::<String>::new()));
    let e = edits.clone();
    f.comp
        .on_field_edited(move |t| e.borrow_mut().push(t.to_string()));
    let commits = count_calls(|g| f.comp.on_field_committed(g));
    f.comp.invoke_focus_code();
    f.key_text(&key(Key::Escape));
    assert_eq!(*esc.borrow(), 0, "no draft: Escape is not the field's");
    f.key_text("zz");
    assert!(!edits.borrow().is_empty(), "typing reports the draft");
    f.key_text(&key(Key::Escape));
    assert_eq!(*esc.borrow(), 1, "a draft: Escape is consumed");
    assert_eq!(*commits.borrow(), 0, "dropping a draft commits nothing");
    // the field is back on the colour: another Escape is not consumed
    f.key_text(&key(Key::Escape));
    assert_eq!(*esc.borrow(), 1);
}

#[test]
fn a_field_commits_on_enter_and_on_losing_focus_only_with_a_draft() {
    let h = make(1.0);
    let commits = count_calls(|g| h.comp.on_field_committed(g));
    h.comp.invoke_focus_code();
    h.key_text("\n");
    assert_eq!(*commits.borrow(), 0, "Enter with no draft commits nothing");
    h.key_text("a");
    h.key_text("\n");
    assert_eq!(*commits.borrow(), 1, "Enter with a draft commits");
    h.key_text("b");
    h.comp.invoke_focus_swatch();
    h.advance(0);
    assert_eq!(
        *commits.borrow(),
        2,
        "losing the focus with a draft commits"
    );
    h.comp.invoke_focus_code();
    h.advance(0);
    h.comp.invoke_focus_swatch();
    h.advance(0);
    assert_eq!(
        *commits.borrow(),
        2,
        "losing the focus with no draft commits nothing"
    );
}

#[test]
fn the_swatch_reports_its_absolute_box() {
    let h = make(2.0);
    let got = Rc::new(RefCell::new(Vec::<(f32, f32, f32, f32)>::new()));
    let g = got.clone();
    h.comp
        .on_field_swatch(move |x, y, w, hh| g.borrow_mut().push((x, y, w, hh)));
    let c = &h.comp;
    h.click(
        c.get_fld_x() + c.get_swatch_x() + 10.0,
        c.get_fld_y() + 10.0,
    );
    let v = got.borrow().clone();
    assert_eq!(v.len(), 1);
    assert!((v[0].0 - (c.get_fld_x() + c.get_swatch_x())).abs() < 0.01);
    assert!(
        (v[0].2 - 36.0).abs() < 1.0 && (v[0].3 - 24.0).abs() < 1.0,
        "{v:?}"
    );
    // the key opens it too
    h.comp.invoke_focus_swatch();
    h.key_text(&key(Key::Return));
    assert_eq!(got.borrow().len(), 2);
}

#[test]
fn a_press_outside_fires_outside_pressed_and_a_press_inside_does_not() {
    let h = open_popover(1.0);
    let outside = count_calls(|f| h.comp.on_outside_pressed(f));
    let c = &h.comp;
    // on the popover's own ground (the left padding), and on the square
    let ground_x = c.get_sv_x() - 6.0;
    h.click(ground_x, c.get_sv_y() + 60.0);
    h.click(c.get_sv_x() + 50.0, c.get_sv_y() + 50.0);
    assert_eq!(
        *outside.borrow(),
        0,
        "presses inside the popover are not outside"
    );
    h.click(5.0, 5.0);
    assert_eq!(*outside.borrow(), 1, "a press elsewhere is outside");
    h.click(400.0, 400.0);
    assert_eq!(*outside.borrow(), 2);
}

#[test]
fn a_press_on_the_ground_does_not_steal_the_focus() {
    let h = open_popover(1.0);
    let log = log_keys(&h);
    let left = count_calls(|f| h.comp.on_focus_left(f));
    h.comp.invoke_focus_popover();
    let c = &h.comp;
    h.click(c.get_sv_x() - 6.0, c.get_sv_y() + 60.0);
    h.key_text(&key(Key::LeftArrow));
    assert_eq!(log.borrow().len(), 1, "the square still has the keys");
    assert_eq!(*left.borrow(), 0, "the focus never left the popover");
}

#[test]
fn the_focus_moving_outside_fires_focus_left() {
    let h = open_popover(1.0);
    let left = count_calls(|f| h.comp.on_focus_left(f));
    h.comp.invoke_focus_popover();
    h.advance(0);
    h.comp.invoke_focus_code();
    h.advance(0);
    assert_eq!(*left.borrow(), 1);
}

#[test]
fn the_eyedropper_button_is_absent_without_a_dropper() {
    let h = open_popover(1.0);
    let dropper = count_calls(|f| h.comp.on_dropper_clicked(f));
    let c = &h.comp;
    let (bx, by) = (
        c.get_drop_x() + c.get_drop_w() / 2.0,
        c.get_foot_y() + c.get_foot_h() / 2.0,
    );
    let chip_with = c.get_chip_x();
    h.click(bx, by);
    assert_eq!(
        *dropper.borrow(),
        1,
        "the button works when a dropper is on"
    );
    h.comp.set_dropper_on(false);
    h.advance(0);
    h.click(bx, by);
    assert_eq!(*dropper.borrow(), 1, "no dropper: nothing to press");
    // the before and after chip takes the room
    assert!(
        h.comp.get_chip_x() < chip_with,
        "the chip starts further left"
    );
    // and the pixels there are the chip's, not a button's fill
    let f = h.render();
    let p = at(&f, 1.0, bx, by);
    assert!(
        Frame::near(p, [0x00, 0xff, 0x00, 255], 4) || Frame::near(p, [0xff, 0x00, 0x00, 255], 4),
        "the chip shows a colour where the button was: {p:?}"
    );
}

#[test]
fn the_alpha_bar_is_inert_and_faded_when_disabled() {
    let h = open_popover(1.0);
    let alpha = Rc::new(RefCell::new(0u32));
    let a = alpha.clone();
    h.comp.on_alpha_moved(move |_, _| *a.borrow_mut() += 1);
    let log = log_keys(&h);
    let c = &h.comp;
    let (ax, ay, aw, ah) = (
        c.get_alpha_x(),
        c.get_alpha_y(),
        c.get_alpha_w(),
        c.get_alpha_h(),
    );
    let on = h.render();
    h.click(ax + aw / 2.0, ay + ah / 2.0);
    assert_eq!(*alpha.borrow(), 1, "an enabled bar answers a press");

    h.comp.set_alpha_disabled(true);
    h.advance(0);
    h.click(ax + aw / 2.0, ay + ah / 2.0);
    assert_eq!(*alpha.borrow(), 1, "a disabled bar answers nothing");
    // its keys are inert too: focus the bar by Tab, press an arrow
    h.comp.invoke_focus_popover();
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::Tab));
    h.key_text(&key(Key::RightArrow));
    assert!(
        log.borrow().iter().all(|k| k.0 != 2),
        "no alpha key: {:?}",
        log.borrow()
    );

    // faded: a pixel on the bar sits closer to the popover's own ground than it did
    let off = h.render();
    let ground: [i32; 4] = [0x1f, 0x1f, 0x1e, 255];
    let dist = |p: [u8; 4]| -> i32 { (0..3).map(|i| (i32::from(p[i]) - ground[i]).abs()).sum() };
    let (px, py) = (ax + 20.0, ay + ah / 2.0);
    assert!(
        dist(at(&off, 1.0, px, py)) < dist(at(&on, 1.0, px, py)),
        "the disabled bar is 40 percent: {:?} vs {:?}",
        at(&off, 1.0, px, py),
        at(&on, 1.0, px, py)
    );
}

#[test]
fn the_popover_buttons_and_chip_carry_prism_terminals_tooltips() {
    let h = open_popover(1.0);
    let c = &h.comp;
    h.move_to(c.get_fmt_x() + 5.0, c.get_foot_y() + 5.0);
    assert_eq!(c.get_last_tip(), "How colour codes are written");
    h.move_to(c.get_chip_x() + 5.0, c.get_foot_y() + 5.0);
    assert_eq!(c.get_last_tip(), "Before and after");
    h.move_to(c.get_drop_x() + 5.0, c.get_foot_y() + 5.0);
    assert_eq!(c.get_last_tip(), "Pick a colour from the screen");
}
