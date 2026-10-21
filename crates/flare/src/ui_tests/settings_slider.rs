//! UI tests of the slider (M7 Task 32); headless only. The preview is a 260 by 40 window with
//! the slider filling it, so its out properties are the slider's own (logical px, device-snapped).

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::{Key, PointerEventButton};

use crate::SliderPreview;
use crate::ui_harness::{Frame, Harness, SCALES};

/// `Theme.ground`: the preview's window colour.
const GROUND: [u8; 4] = [23, 23, 23, 255];

fn make(scale: f32) -> Harness<SliderPreview> {
    let h = Harness::new(scale, 260, 40, || {
        SliderPreview::new().expect("the preview builds")
    });
    h.advance(0);
    h
}

fn set(h: &Harness<SliderPreview>, value: i32, max: i32) {
    h.comp.set_max(max);
    h.comp.set_value(value);
}

/// Records every `changed` call, in order.
fn log_changes(h: &Harness<SliderPreview>) -> Rc<RefCell<Vec<i32>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    h.comp.on_changed(move |v| l.borrow_mut().push(v));
    log
}

/// True when the logical value `v` lands on a whole device pixel at scale `s`.
fn whole(v: f32, s: f32) -> bool {
    let d = v * s;
    (d - d.round()).abs() < 1e-2
}

#[test]
fn track_168_by_6_thumb_18_value_cell_38_at_every_scale() {
    for s in SCALES {
        let h = make(s);
        set(&h, 0, 70);
        let c = &h.comp;
        let edges = [
            c.get_tr_x(),
            c.get_tr_x() + c.get_tr_w(),
            c.get_th_x(),
            c.get_th_x() + c.get_th_w(),
            c.get_val_x(),
            c.get_val_x() + c.get_val_w(),
        ];
        for e in edges {
            assert!(whole(e, s), "scale {s}: edge {e} is off a device pixel");
        }
        assert!(
            (c.get_tr_w() * s - 168.0 * s).abs() <= 1.0,
            "scale {s}: track width"
        );
        assert!(
            (c.get_tr_h() * s - 6.0 * s).abs() <= 1.0,
            "scale {s}: track height"
        );
        assert!(
            (c.get_th_w() * s - 18.0 * s).abs() <= 1.0,
            "scale {s}: thumb width"
        );
        assert!(
            (c.get_val_w() * s - 38.0 * s).abs() <= 1.0,
            "scale {s}: value cell"
        );
        // the value cell sits 12 px after the track
        assert!(
            (c.get_val_x() * s - 180.0 * s).abs() <= 1.0,
            "scale {s}: value cell x"
        );
    }
}

#[test]
fn thumb_centre_follows_the_value() {
    for s in SCALES {
        let h = make(s);
        // 0, half and the top of a range of 70: the centre runs 9, 84, 159 px (9 + pct * 150)
        for (value, centre) in [(0, 9.0), (35, 84.0), (70, 159.0)] {
            set(&h, value, 70);
            let c = &h.comp;
            let mid = c.get_th_x() + c.get_th_w() / 2.0;
            assert!(
                (mid * s - centre * s).abs() <= 1.0,
                "scale {s}, value {value}: centre {mid} want {centre}"
            );
        }
    }
}

#[test]
fn a_press_and_a_drag_set_the_value_and_fire_once_per_change() {
    let h = make(1.0);
    set(&h, 0, 70);
    let log = log_changes(&h);
    // a press at x = 9 + 0.5 * 150 = 84 sets round(0.5 * 70) = 35
    h.move_to(84.0, 20.0);
    h.press(84.0, 20.0, PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec![35]);
    // a drag to x = 99 gives 42; a second move inside the same value fires nothing
    h.move_to(99.0, 20.0);
    assert_eq!(*log.borrow(), vec![35, 42]);
    h.move_to(99.5, 20.0);
    assert_eq!(*log.borrow(), vec![35, 42]);
    // the drag reaches the top
    h.move_to(159.0, 20.0);
    h.release(159.0, 20.0, PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec![35, 42, 70]);
}

#[test]
fn right_steps_one_and_stops_at_the_top() {
    let h = make(1.0);
    set(&h, 30, 70);
    let log = log_changes(&h);
    h.comp.invoke_focus_slider();
    let right = String::from(char::from(Key::RightArrow));
    for _ in 0..45 {
        h.key_text(&right);
        // the store writes each value back, as settings_ctl does
        let last = log.borrow().last().copied();
        if let Some(v) = last {
            h.comp.set_value(v);
        }
    }
    assert_eq!(*log.borrow(), (31..=70).collect::<Vec<i32>>());
    assert_eq!(h.comp.get_value(), 70);
}

#[test]
fn the_value_reads_off_at_zero_by_the_text_it_is_given() {
    let h = make(1.0);
    let ink_in_cell = |f: &Frame| f.count_where(180, 0, 218, 40, |p| p != GROUND);
    h.comp.set_value(0);
    h.comp.set_text("Off".into());
    assert_eq!(h.comp.get_val_text().as_str(), "Off");
    let off = ink_in_cell(&h.render());
    assert!(off > 0, "the cell paints the text");
    h.comp.set_text("30%".into());
    assert_eq!(h.comp.get_val_text().as_str(), "30%");
    assert!(ink_in_cell(&h.render()) > 0);
}

#[test]
fn a_disabled_slider_ignores_input_and_is_dimmed() {
    let h = make(1.0);
    set(&h, 70, 70);
    // enabled: the thumb at the top is white
    let lit = h.render().px(159, 20);
    assert!(Frame::near(lit, [255, 255, 255, 255], 3), "{lit:?}");
    h.comp.set_disabled(true);
    let log = log_changes(&h);
    h.comp.invoke_focus_slider();
    h.move_to(84.0, 20.0);
    h.press(84.0, 20.0, PointerEventButton::Left);
    h.release(84.0, 20.0, PointerEventButton::Left);
    let left = String::from(char::from(Key::LeftArrow));
    h.key_text(&left);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(h.comp.get_value(), 70);
    // dimmed: the white thumb reads at half opacity over the ground, about 139
    let dim = h.render().px(159, 20);
    assert!((120..=160).contains(&dim[0]), "{dim:?}");
}
