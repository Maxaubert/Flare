//! A plugin's secret question with an error line (`secret.ask` `error`, rule 1266, the approved mockup C), headless:
//! the error stands in the sub's place in the error colour, the field gets a red edge and a soft red glow, it is empty
//! and has the keys; with no error the dialog looks as it did. The box ends at Cancel: no line under it (2026-11-16).

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, PluginLook};

/// `SettingsFixed.error-dark` and `BrandInk.error`: the error line's ink on a dark ground.
const ERROR_INK: [u8; 4] = [255, 138, 141, 255];
/// `SettingsFixed.error-edge` and `BrandInk.error-edge`: the field's red edge.
const ERROR_EDGE: [u8; 4] = [229, 72, 77, 255];

fn branded() -> PluginLook {
    PluginLook {
        branded: true,
        accent: slint::Color::from_rgb_u8(23, 93, 220),
        accent_hi: slint::Color::from_rgb_u8(106, 153, 240),
        logo: "M4 4h16v16H4z".into(),
        plate: true,
    }
}

fn ask(s: f32, look: PluginLook, error: &str) -> Harness<AppWindow> {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.comp.set_pask_kind(1);
    h.comp.set_pask_title("Your vault is locked".into());
    h.comp.set_pask_sub("me@example.com".into());
    h.comp.set_pask_error(error.into());
    h.comp.set_pask_label("Master password".into());
    h.comp.set_pask_button("Unlock".into());
    h.comp.set_pask_look(look);
    h.comp.set_pask_open(true);
    h.advance(0);
    h
}

/// Pixels of the error ink between the box's top and the field (the title, the sub line).
fn error_ink_above_field(h: &Harness<AppWindow>, f: &Frame, s: f32) -> usize {
    let c = &h.comp;
    let x0 = (c.get_pask_box_x() * s) as u32;
    let x1 = ((c.get_pask_box_x() + c.get_pask_box_w()) * s) as u32;
    let y0 = (c.get_pask_box_y() * s) as u32;
    let y1 = ((c.get_pask_field_y() - 4.0) * s) as u32;
    f.count_where(x0, y0, x1, y1, |p| Frame::near(p, ERROR_INK, 24))
}

/// Pixels of the red edge along the field's left side, at mid height.
fn red_edge_left(h: &Harness<AppWindow>, f: &Frame, s: f32) -> usize {
    let c = &h.comp;
    let x = c.get_pask_field_x() * s;
    let ym = (c.get_pask_field_y() + c.get_pask_field_h() / 2.0) * s;
    f.count_where(
        (x - 1.0) as u32,
        (ym - 6.0) as u32,
        (x + 2.0) as u32,
        (ym + 6.0) as u32,
        |p| Frame::near(p, ERROR_EDGE, 36),
    )
}

/// The glow's pixel 2 css px outside the field's left edge is red: red above green, green and blue alike (the
/// usual focus glow, the accent's tint, has green well above blue).
fn red_glow_left(h: &Harness<AppWindow>, f: &Frame, s: f32) -> bool {
    let c = &h.comp;
    let x = ((c.get_pask_field_x() - 2.0) * s) as u32;
    let y = ((c.get_pask_field_y() + c.get_pask_field_h() / 2.0) * s) as u32;
    let p = f.px(x, y);
    let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
    r > g + 12 && (g - b).abs() <= 8
}

#[test]
fn an_error_is_red_in_the_sub_place_and_the_field_gets_a_red_edge() {
    for s in SCALES {
        for look in [crate::plugin_look::look_of(None), branded()] {
            let h = ask(s, look.clone(), "Wrong password. Try again.");
            let f = h.render();
            assert!(h.comp.get_pask_error_shown(), "scale {s}");
            assert!(
                error_ink_above_field(&h, &f, s) > 20,
                "the error line is drawn in the error ink at scale {s} (branded {})",
                look.branded
            );
            assert!(
                red_edge_left(&h, &f, s) > 0,
                "the field has a red edge at scale {s} (branded {})",
                look.branded
            );
            assert!(
                red_glow_left(&h, &f, s),
                "the field has a red glow at scale {s} (branded {})",
                look.branded
            );
            // The field is empty and holds the keys, so the person types again at once.
            assert!(h.comp.get_pask_field_focus(), "scale {s}");
        }
    }
}

#[test]
fn no_error_keeps_todays_look_and_the_error_does_not_move_the_box() {
    for s in SCALES {
        let plain = ask(s, crate::plugin_look::look_of(None), "");
        let f = plain.render();
        assert!(!plain.comp.get_pask_error_shown());
        assert_eq!(error_ink_above_field(&plain, &f, s), 0, "scale {s}");
        assert_eq!(red_edge_left(&plain, &f, s), 0, "scale {s}");
        assert!(!red_glow_left(&plain, &f, s), "scale {s}");
        assert!(plain.comp.get_pask_field_focus());
        // The error takes the sub's line: the box keeps its size and the field its place.
        let err = ask(
            s,
            crate::plugin_look::look_of(None),
            "Wrong password. Try again.",
        );
        let _ = err.render();
        assert_eq!(
            plain.comp.get_pask_box_h(),
            err.comp.get_pask_box_h(),
            "scale {s}"
        );
        assert_eq!(
            plain.comp.get_pask_field_y(),
            err.comp.get_pask_field_y(),
            "scale {s}"
        );
    }
}

#[test]
fn the_secret_box_ends_at_cancel_with_no_line_under_it() {
    // Max took the "Asked by <plugin>" line out (2026-11-16, rule 1266): the box ends 16 px under Cancel, its bottom
    // padding, and nothing is drawn between.
    for s in SCALES {
        for look in [crate::plugin_look::look_of(None), branded()] {
            let h = ask(s, look.clone(), "");
            let f = h.render();
            let c = &h.comp;
            let at = format!("scale {s} (branded {})", look.branded);
            let cancel_bottom = c.get_pask_deny_y() + c.get_pask_deny_h();
            let box_bottom = c.get_pask_box_y() + c.get_pask_box_h();
            assert!(
                (box_bottom - cancel_bottom - 16.0).abs() <= 1.0,
                "gap {} at {at}",
                box_bottom - cancel_bottom
            );
            // The strip under Cancel, inside the box's rounded corners, is the box's own ground only.
            let (x0, x1) = (
                ((c.get_pask_box_x() + 16.0) * s) as u32,
                ((c.get_pask_box_x() + c.get_pask_box_w() - 16.0) * s) as u32,
            );
            let (y0, y1) = (
                ((cancel_bottom + 2.0) * s) as u32,
                ((box_bottom - 4.0) * s) as u32,
            );
            let bg = f.px(x0, y0);
            assert_eq!(
                f.count_where(x0, y0, x1, y1, |p| !Frame::near(p, bg, 12)),
                0,
                "nothing drawn under Cancel at {at}"
            );
        }
    }
}
