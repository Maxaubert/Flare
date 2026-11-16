//! A checked secret's dialog (`secret.ask` with `verify`, rule 1274, Max's approved mockup), headless at every scale
//! and in both looks: after Unlock the box stays open, checking (the field read-only and dimmed, keeping its dots; the
//! button off with a turning spinner; the checking line in the sub's place; Cancel still works). A right answer shows a
//! green check with its heading and line and no field; a wrong one comes back to the field in place, cleared, holding
//! the keys, with the red line; an end shows the plugin's logo and its plain line. Without `verify` the field is
//! cleared on send, as before.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, PluginLook};

/// `SettingsFixed.ok-dark` and `BrandInk.ok`: the check's green on a dark ground.
const OK_INK: [u8; 4] = [82, 194, 122, 255];
/// `SettingsFixed.error-dark` and `BrandInk.error`: the error line's ink.
const ERROR_INK: [u8; 4] = [255, 138, 141, 255];

fn branded() -> PluginLook {
    PluginLook {
        branded: true,
        accent: slint::Color::from_rgb_u8(23, 93, 220),
        accent_hi: slint::Color::from_rgb_u8(106, 153, 240),
        logo: "M4 4h16v16H4z".into(),
        plate: true,
    }
}

fn looks() -> [PluginLook; 2] {
    [crate::plugin_look::look_of(None), branded()]
}

/// What the dialog handed to the glue: (ok, text).
type Sent = Rc<RefCell<Vec<(bool, String)>>>;

fn ask(s: f32, look: PluginLook, verify: bool) -> (Harness<AppWindow>, Sent) {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    let sent: Sent = Rc::default();
    {
        let sent = Rc::clone(&sent);
        h.comp
            .on_pask_secret(move |ok, t| sent.borrow_mut().push((ok, t.to_string())));
    }
    h.comp.set_pask_kind(1);
    h.comp.set_pask_title("Your vault is locked".into());
    h.comp.set_pask_sub("me@example.com".into());
    h.comp.set_pask_label("Master password".into());
    h.comp.set_pask_button("Unlock".into());
    h.comp.set_pask_look(look);
    h.comp.set_pask_verify(verify);
    h.comp.set_pask_checking("Checking the password...".into());
    h.comp.set_pask_stage(0);
    h.comp.set_pask_open(true);
    h.activate();
    h.advance(0);
    // Laid out at the window's size (the title wraps to the box's width).
    let _ = h.render();
    (h, sent)
}

/// Types into the field and presses the button with the pointer, as the person does.
fn type_and_unlock(h: &Harness<AppWindow>) {
    h.key_text("hunter2");
    h.advance(0);
    let c = &h.comp;
    h.click(
        c.get_pask_ok_x() + c.get_pask_ok_w() / 2.0,
        c.get_pask_ok_y() + c.get_pask_ok_h() / 2.0,
    );
    h.advance(0);
}

/// The square of the button's middle where the spinner turns (device px).
fn spinner_square(h: &Harness<AppWindow>, s: f32) -> (u32, u32, u32, u32) {
    let c = &h.comp;
    let cx = c.get_pask_ok_x() + c.get_pask_ok_w() / 2.0;
    let cy = c.get_pask_ok_y() + c.get_pask_ok_h() / 2.0;
    (
        ((cx - 10.0) * s) as u32,
        ((cy - 10.0) * s) as u32,
        ((cx + 10.0) * s) as u32,
        ((cy + 10.0) * s) as u32,
    )
}

/// How many pixels of the square differ between two frames.
fn changed(a: &Frame, b: &Frame, sq: (u32, u32, u32, u32)) -> usize {
    let (x0, y0, x1, y1) = sq;
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if !Frame::near(a.px(x, y), b.px(x, y), 16) {
                n += 1;
            }
        }
    }
    n
}

fn green_in_mark(h: &Harness<AppWindow>, f: &Frame, s: f32) -> usize {
    let c = &h.comp;
    let (x, y, w, hh) = (
        c.get_pask_mark_x(),
        c.get_pask_mark_y(),
        c.get_pask_mark_w(),
        c.get_pask_mark_h(),
    );
    f.count_where(
        (x * s) as u32,
        (y * s) as u32,
        ((x + w) * s) as u32,
        ((y + hh) * s) as u32,
        |p| Frame::near(p, OK_INK, 30),
    )
}

#[test]
fn checking_keeps_the_box_with_a_read_only_field_a_spinner_and_cancel() {
    for s in SCALES {
        for look in looks() {
            let at = format!("scale {s} (branded {})", look.branded);
            let (h, sent) = ask(s, look, true);
            let box_h = h.comp.get_pask_box_h();
            let field_y = h.comp.get_pask_field_y();
            type_and_unlock(&h);
            assert_eq!(
                sent.borrow().as_slice(),
                &[(true, "hunter2".to_string())],
                "{at}"
            );
            // The glue moves it to checking; the field keeps its dots.
            h.comp.set_pask_stage(1);
            h.advance(0);
            let c = &h.comp;
            assert!(c.get_pask_open(), "{at}");
            assert!(c.get_pask_checking_shown(), "{at}");
            assert!(c.get_pask_field_read_only(), "{at}");
            assert!(!c.get_pask_ok_enabled(), "{at}");
            assert!(!c.get_pask_field_empty(), "the dots stay at {at}");
            // The box does not move or change size.
            assert_eq!(c.get_pask_box_h(), box_h, "{at}");
            assert_eq!(c.get_pask_field_y(), field_y, "{at}");
            // The spinner turns: its square differs from one frame to the next.
            let f1 = h.render();
            h.advance(150);
            let f2 = h.render();
            assert!(
                changed(&f1, &f2, spinner_square(&h, s)) > 4,
                "the spinner turns at {at}"
            );
            // No red while checking, even with an error from the last try.
            h.comp.set_pask_error("Wrong password, try again".into());
            h.advance(0);
            assert!(!h.comp.get_pask_error_shown(), "{at}");
            // A second press of the off button sends nothing; Cancel still works.
            type_and_unlock(&h);
            assert_eq!(sent.borrow().len(), 1, "{at}");
            let c = &h.comp;
            assert!(c.get_pask_deny_h() > 0.0, "Cancel is there at {at}");
            h.click(
                c.get_pask_deny_x() + c.get_pask_deny_w() / 2.0,
                c.get_pask_deny_y() + c.get_pask_deny_h() / 2.0,
            );
            assert_eq!(
                sent.borrow().last().cloned(),
                Some((false, String::new())),
                "{at}"
            );
        }
    }
}

#[test]
fn wrong_comes_back_in_place_cleared_focused_and_red() {
    for s in SCALES {
        for look in looks() {
            let at = format!("scale {s} (branded {})", look.branded);
            let (h, _sent) = ask(s, look, true);
            let field_y = h.comp.get_pask_field_y();
            type_and_unlock(&h);
            h.comp.set_pask_stage(1);
            h.advance(0);
            h.comp.set_pask_error("Wrong password, try again".into());
            h.comp.set_pask_stage(0);
            h.advance(0);
            let c = &h.comp;
            assert!(c.get_pask_open(), "{at}");
            assert!(c.get_pask_field_empty(), "the field is cleared at {at}");
            assert!(c.get_pask_field_focus(), "it holds the keys at {at}");
            assert!(c.get_pask_error_shown(), "{at}");
            assert!(!c.get_pask_field_read_only(), "{at}");
            assert!(c.get_pask_ok_enabled(), "{at}");
            assert_eq!(c.get_pask_field_y(), field_y, "in place at {at}");
            let f = h.render();
            let (x0, y0, x1, y1) = (
                (c.get_pask_box_x() * s) as u32,
                (c.get_pask_box_y() * s) as u32,
                ((c.get_pask_box_x() + c.get_pask_box_w()) * s) as u32,
                ((c.get_pask_field_y() - 4.0) * s) as u32,
            );
            assert!(
                f.count_where(x0, y0, x1, y1, |p| Frame::near(p, ERROR_INK, 24)) > 20,
                "the red line at {at}"
            );
            // The person types again at once.
            h.key_text("next");
            assert!(!h.comp.get_pask_field_empty(), "{at}");
        }
    }
}

#[test]
fn right_shows_a_green_check_with_its_heading_and_no_field() {
    for s in SCALES {
        for look in looks() {
            let at = format!("scale {s} (branded {})", look.branded);
            let (h, _sent) = ask(s, look, true);
            let input = h.render();
            assert_eq!(green_in_mark(&h, &input, s), 0, "no green before at {at}");
            let box_before = h.comp.get_pask_box_h();
            h.comp.set_pask_result_title("Vault unlocked".into());
            h.comp
                .set_pask_result_line("Bitwarden can fill logins now.".into());
            h.comp.set_pask_stage(2);
            h.advance(0);
            let f = h.render();
            let c = &h.comp;
            assert!(c.get_pask_result_shown(), "{at}");
            assert!(green_in_mark(&h, &f, s) > 20, "the green check at {at}");
            assert_eq!(c.get_pask_field_h(), 0.0, "no field at {at}");
            assert_eq!(c.get_pask_foot_h(), 0.0, "no Cancel and no foot at {at}");
            assert!(c.get_pask_box_h() < box_before, "{at}");
            // The heading and the line are drawn under the check.
            let (bx, by, bw) = (c.get_pask_box_x(), c.get_pask_box_y(), c.get_pask_box_w());
            let mark_bottom = c.get_pask_mark_y() + c.get_pask_mark_h();
            let bg = f.px(((bx + 4.0) * s) as u32, ((mark_bottom + 20.0) * s) as u32);
            assert!(
                f.count_where(
                    (bx * s) as u32,
                    (mark_bottom * s) as u32,
                    ((bx + bw) * s) as u32,
                    ((by + c.get_pask_box_h()) * s) as u32,
                    |p| !Frame::near(p, bg, 24)
                ) > 60,
                "the words at {at}"
            );
        }
    }
}

#[test]
fn an_end_shows_the_logo_and_the_plain_line() {
    for s in SCALES {
        for look in looks() {
            let at = format!("scale {s} (branded {})", look.branded);
            let (h, _sent) = ask(s, look, true);
            h.comp.set_pask_result_title("Your vault is locked".into());
            h.comp
                .set_pask_result_line("Bitwarden did not unlock after 5 tries.".into());
            h.comp.set_pask_stage(3);
            h.advance(0);
            let f = h.render();
            let c = &h.comp;
            assert!(c.get_pask_result_shown(), "{at}");
            assert_eq!(green_in_mark(&h, &f, s), 0, "no check at {at}");
            assert_eq!(c.get_pask_field_h(), 0.0, "{at}");
            assert!(!c.get_pask_error_shown(), "{at}");
        }
    }
}

#[test]
fn without_verify_the_field_is_cleared_on_send_as_before() {
    for s in SCALES {
        let (h, sent) = ask(s, crate::plugin_look::look_of(None), false);
        type_and_unlock(&h);
        assert_eq!(sent.borrow().len(), 1);
        assert!(h.comp.get_pask_field_empty(), "scale {s}");
        assert!(!h.comp.get_pask_checking_shown());
        assert!(h.comp.get_pask_ok_enabled());
    }
}
