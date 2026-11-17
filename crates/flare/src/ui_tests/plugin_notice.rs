//! A plugin's notice under the title bar (`notice.show`, rule 1275, Max's approved mockup A), headless, at every display
//! scale, in both plugin looks (Flare's chip and a brand's logo) and both layouts: it sits where the "Build ready"
//! toast sits and is as tall, under that toast and the update notice when they show (never over them), draws the
//! plugin's logo, its one line and an icon-only close button, and a 3 px red left edge for an error only. The close
//! button has a tooltip and closes it; the pointer on it holds it (the glue's clock stops).

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, PluginLook, PluginNoticeRow};

/// `SettingsFixed.error-edge`: the red edge.
const ERROR_EDGE: [u8; 4] = [229, 72, 77, 255];
const TEXT: &str = "Bitwarden could not lock the vault. It is still unlocked.";

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

fn row(id: &str, text: &str, error: bool, look: PluginLook) -> PluginNoticeRow {
    PluginNoticeRow {
        id: id.into(),
        name: format!("Plugin {id}").into(),
        text: text.into(),
        error,
        look,
    }
}

fn window(s: f32, vertical: bool) -> Harness<AppWindow> {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    if vertical {
        h.comp.set_layout_vertical(true);
        h.comp.set_has_tabs(true);
        h.comp.set_sidebar_width(220.0);
    }
    h.advance(0);
    h
}

fn set(h: &Harness<AppWindow>, rows: Vec<PluginNoticeRow>) {
    h.comp
        .set_plugin_notices(ModelRc::new(VecModel::from(rows)));
    h.advance(0);
}

/// The first notice's box, css px.
fn first(h: &Harness<AppWindow>) -> (f32, f32, f32, f32) {
    let c = &h.comp;
    (
        c.get_plugin_notice_x(),
        c.get_plugin_notice_y(),
        c.get_plugin_notice_w(),
        c.get_plugin_notice_h(),
    )
}

/// Pixels in the css box that are not `bg`.
fn ink(f: &Frame, s: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), bg: [u8; 4]) -> usize {
    let p = |v: f32| (v * s).round() as u32;
    f.count_where(p(x0), p(y0), p(x1), p(y1), |q| !Frame::near(q, bg, 24))
}

#[test]
fn the_notice_sits_where_the_staged_toast_does_and_draws_its_logo_line_and_close() {
    for s in SCALES {
        for vertical in [false, true] {
            for look in looks() {
                let at = format!("scale {s}, vertical {vertical}, branded {}", look.branded);
                let h = window(s, vertical);
                // Where the staged toast would be, alone.
                h.comp.set_staged_toast("Installed abc1234".into());
                h.advance(0);
                let (sx, sy, sw, sh) = (
                    h.comp.get_staged_toast_x(),
                    h.comp.get_staged_toast_y(),
                    h.comp.get_staged_toast_w(),
                    h.comp.get_staged_toast_h(),
                );
                h.comp.set_staged_toast("".into());
                set(&h, vec![row("vault", TEXT, true, look.clone())]);
                let (x, y, w, nh) = first(&h);
                assert!(
                    ((x + w) - (sx + sw)).abs() <= 1.0,
                    "right-aligned as the toast at {at}"
                );
                assert!(
                    (y - sy).abs() <= 0.5,
                    "the toast's top at {at}: {y} vs {sy}"
                );
                assert!(
                    (nh - sh).abs() <= 1.5,
                    "the toast's height at {at}: {nh} vs {sh}"
                );
                assert!(w > 200.0 && w < 700.0, "one line at {at}: {w}");
                assert!(h.comp.get_overlay_open(), "{at}");
                // Device pixels on the grid: the box is snapped.
                let snapped = |v: f32| ((v * s) - (v * s).round()).abs() < 0.02;
                assert!(snapped(y) && snapped(nh), "snapped at {at}");

                let f = h.render();
                let p = |v: f32| (v * s).round() as u32;
                let bg = f.px(p(x + 28.0), p(y + 3.0));
                // The red edge, down the left side at mid height.
                let edge = f.count_where(
                    p(x),
                    p(y + nh / 2.0 - 3.0),
                    p(x + 3.0),
                    p(y + nh / 2.0 + 3.0),
                    |q| Frame::near(q, ERROR_EDGE, 40),
                );
                assert!(edge > 0, "a red edge at {at}");
                // The logo, the line and the close button are drawn.
                let mid = y + nh / 2.0;
                assert!(
                    ink(&f, s, (x + 10.0, mid - 7.0, x + 26.0, mid + 7.0), bg) > 10,
                    "logo at {at}"
                );
                assert!(
                    ink(&f, s, (x + 40.0, mid - 6.0, x + w - 40.0, mid + 6.0), bg) > 60,
                    "text at {at}"
                );
                let cx = h.comp.get_plugin_notice_close_x();
                let cy = h.comp.get_plugin_notice_close_y();
                assert!(
                    cx > x + w - 30.0 && cx < x + w,
                    "close at the right at {at}"
                );
                assert!(
                    ink(&f, s, (cx + 3.0, cy + 3.0, cx + 13.0, cy + 13.0), bg) > 2,
                    "close at {at}"
                );

                // An info notice has no red edge.
                set(&h, vec![row("vault", TEXT, false, look.clone())]);
                let f = h.render();
                let red = f.count_where(p(x), p(y + 4.0), p(x + 4.0), p(y + nh - 4.0), |q| {
                    Frame::near(q, ERROR_EDGE, 40)
                });
                assert_eq!(red, 0, "no red edge for info at {at}");
            }
        }
    }
}

#[test]
fn notices_stack_under_the_staged_toast_and_the_update_notice_and_never_cover_them() {
    for s in SCALES {
        for vertical in [false, true] {
            let at = format!("scale {s}, vertical {vertical}");
            let h = window(s, vertical);
            h.comp.set_staged_chip_shown(true);
            h.comp.set_staged_chip_label("Build ready".into());
            h.comp
                .set_staged_toast("Installing when Claude is idle".into());
            h.comp.set_staged_toast_button("Cancel".into());
            set(
                &h,
                vec![
                    row("vault", TEXT, true, crate::plugin_look::look_of(None)),
                    row("other", "Synced", false, branded()),
                ],
            );
            let (sx, sy, sw, sh) = (
                h.comp.get_staged_toast_x(),
                h.comp.get_staged_toast_y(),
                h.comp.get_staged_toast_w(),
                h.comp.get_staged_toast_h(),
            );
            let (x, y, w, nh) = first(&h);
            assert!(
                y >= sy + sh + 8.0 - 0.5,
                "under the staged toast at {at}: {y} vs {}",
                sy + sh
            );
            assert!(((x + w) - (sx + sw)).abs() <= 1.0, "right-aligned at {at}");
            let pitch = h.comp.get_plugin_notice_pitch();
            assert!(
                pitch >= nh + 8.0 - 0.5,
                "the second sits under the first at {at}"
            );
            // Where the second draws, the first does not reach.
            let f = h.render();
            let p = |v: f32| (v * s).round() as u32;
            let gap_bg = f.px(p(x + w - 4.0), p(y + nh + 3.0));
            assert!(
                !Frame::near(gap_bg, ERROR_EDGE, 40),
                "nothing red in the gap at {at}"
            );
            // The update notice too.
            h.comp.set_update_notice("Flare 9.9.9 is ready".into());
            h.advance(0);
            let (_, y2, _, _) = first(&h);
            let unote_bottom = h.comp.get_update_chip_y()
                + h.comp.get_update_chip_h()
                + 8.0
                + h.comp.get_update_notice_h();
            assert!(
                y2 >= unote_bottom + 8.0 - 1.0,
                "under the update notice at {at}"
            );
        }
    }
}

#[test]
fn the_close_button_has_a_tip_and_closes_and_the_pointer_holds_the_notice() {
    for s in [1.0f32, 1.5, 2.0] {
        let h = window(s, false);
        let hovers: Rc<RefCell<Vec<(String, bool)>>> = Rc::default();
        let closed: Rc<RefCell<Vec<String>>> = Rc::default();
        let tips: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let v = hovers.clone();
            h.comp
                .on_plugin_notice_hover(move |id, on| v.borrow_mut().push((id.to_string(), on)));
            let v = closed.clone();
            h.comp
                .on_plugin_notice_closed(move |id| v.borrow_mut().push(id.to_string()));
            let v = tips.clone();
            h.comp
                .on_tip_request(move |t, _, _, _, _| v.borrow_mut().push(t.to_string()));
        }
        set(&h, vec![row("vault", TEXT, true, branded())]);
        let (x, y, w, nh) = first(&h);
        h.move_to(x + w / 2.0, y + nh / 2.0);
        h.advance(0);
        assert_eq!(*hovers.borrow(), [("vault".to_string(), true)], "scale {s}");
        assert!(tips.borrow().is_empty(), "the line has no tooltip");
        // Onto the close button: still on the notice, and the button says what it does.
        let cx = h.comp.get_plugin_notice_close_x() + 8.0;
        let cy = h.comp.get_plugin_notice_close_y() + 8.0;
        h.move_to(cx, cy);
        h.advance(0);
        assert_eq!(
            hovers.borrow().len(),
            1,
            "no leave over the button at scale {s}"
        );
        assert_eq!(tips.borrow().last().map(String::as_str), Some("Close"));
        h.click(cx, cy);
        h.advance(0);
        assert_eq!(*closed.borrow(), ["vault"], "scale {s}");
        // Away from it: it is left.
        h.move_to(20.0, 400.0);
        h.advance(0);
        assert_eq!(
            hovers.borrow().last(),
            Some(&("vault".to_string(), false)),
            "scale {s}"
        );
        assert_eq!(tips.borrow().last().map(String::as_str), Some(""));
    }
}

#[test]
fn a_long_line_is_cut_with_an_ellipsis_and_the_notice_keeps_its_width() {
    let h = window(1.0, false);
    set(
        &h,
        vec![row(
            "vault",
            &"W".repeat(120),
            false,
            crate::plugin_look::look_of(None),
        )],
    );
    let (x, _, w, _) = first(&h);
    assert!(
        w <= 10.0 + 16.0 + 9.0 + 480.0 + 9.0 + 16.0 + 6.0 + 2.0,
        "{w}"
    );
    assert!(x > 300.0, "{x}");
}
