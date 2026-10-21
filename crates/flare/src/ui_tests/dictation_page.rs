//! Pixel and geometry tests of the Dictation page controls at all eight scalings (M8 Task 25); headless only.
//!
//! `DictationPageControls` is the key field, the microphone, the language mark, a panel of three model rows
//! (y 160 in css px, each 64 high) and the one-line question, in a 640 by 520 window. Every size is read from the
//! component's out properties or from the rendered pixels; callbacks are recorded into its `log`.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{DictationPageControls, SetButton, SetRow, Theme};

const NAME: &str = "Whisper Base";
const LINE: &str = "This model doesn't support all languages.";

fn dev(scale: f32, css: f32) -> u32 {
    (css * scale).round() as u32
}

fn button(id: &str, label: &str) -> SetButton {
    SetButton {
        id: id.into(),
        label: label.into(),
        label_alt: SharedString::new(),
        off: false,
        alt: false,
    }
}

fn model(id: &str, state: &str, buttons: Vec<SetButton>) -> SetRow {
    SetRow {
        id: id.into(),
        label: NAME.into(),
        sub: "Fast live text on any PC.".into(),
        vendor: "openai".into(),
        size_text: "148 MB".into(),
        state: state.into(),
        buttons: ModelRc::new(VecModel::from(buttons)),
        ..Default::default()
    }
}

/// The three rows: the one in use, a download at 50 percent, and one not yet downloaded (Recommended).
fn harness(scale: f32) -> Harness<DictationPageControls> {
    let h = Harness::new(scale, 640, 520, || DictationPageControls::new().unwrap());
    let mut a = model("base", "active", vec![button("uninstall", "Uninstall")]);
    a.badge = "Active".into();
    a.button_title = "Delete Whisper Base from this PC.".into();
    let mut b = model("small", "downloading", vec![button("cancel", "Cancel")]);
    b.num = 50;
    let mut c = model("tiny", "absent", vec![button("download", "Download")]);
    c.recommended = true;
    h.comp.set_row_a(a);
    h.comp.set_row_b(b);
    h.comp.set_row_c(c);
    h.advance(0);
    h
}

fn log(h: &Harness<DictationPageControls>) -> String {
    h.comp.get_log().to_string()
}

#[test]
fn the_download_bar_is_6_by_112() {
    for scale in SCALES {
        let h = harness(scale);
        let t = h.comp.global::<Theme>();
        let track = t.get_track();
        let hi = t.get_accent_hi();
        let track = [track.red(), track.green(), track.blue(), 255];
        let hi = [hi.red(), hi.green(), hi.blue(), 255];
        let f = h.render();
        let row_y = h.comp.get_b_y();
        // The bar sits at the left of the row's controls, centred on the row.
        let left = h.comp.get_panel_x() + 1.0 + h.comp.get_b_right_x();
        let x0 = dev(scale, left);
        let bar = f
            .bbox_where(
                x0,
                dev(scale, row_y + 28.0),
                dev(scale, left + 130.0),
                dev(scale, row_y + 36.0),
                |p| Frame::near(p, track, 3) || Frame::near(p, hi, 3),
            )
            .expect("the bar is drawn");
        let (w, hgt) = (bar.2 - bar.0 + 1, bar.3 - bar.1 + 1);
        assert!(
            w.abs_diff(dev(scale, 112.0)) <= 2,
            "scale {scale}: bar width {w}"
        );
        assert!(
            hgt.abs_diff(dev(scale, 6.0)) <= 1,
            "scale {scale}: bar height {hgt}"
        );
        // 50 percent: half of the track is the fill.
        let filled = f
            .bbox_where(bar.0, bar.1, bar.2 + 1, bar.3 + 1, |p| {
                Frame::near(p, hi, 3)
            })
            .expect("the fill is drawn");
        let fw = filled.2 - filled.0 + 1;
        assert!(
            fw.abs_diff(dev(scale, 56.0)) <= 2,
            "scale {scale}: fill width {fw}"
        );
    }
}

#[test]
fn the_vendor_mark_is_left_and_centred() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        assert!(
            c.get_a_mark_x() < c.get_a_name_x(),
            "scale {scale}: the mark ({}) sits left of the name ({})",
            c.get_a_mark_x(),
            c.get_a_name_x()
        );
        assert!(
            c.get_a_mark_w() >= 28.0 && (c.get_a_mark_w() - 36.0).abs() < 0.01,
            "scale {scale}: mark {}",
            c.get_a_mark_w()
        );
        let centre = c.get_a_mark_y() + c.get_a_mark_h() / 2.0;
        assert!(
            (centre - c.get_a_h() / 2.0).abs() <= 1.0 / scale + 0.01,
            "scale {scale}: the mark's centre {centre} against the row's {}",
            c.get_a_h() / 2.0
        );
        // The glyph is drawn: ink inside the 36 square.
        let f = h.render();
        let text = c.global::<Theme>().get_text();
        let ink = [text.red(), text.green(), text.blue(), 255];
        let x = dev(scale, c.get_panel_x() + 1.0 + c.get_a_mark_x());
        let y = dev(scale, c.get_a_y() + c.get_a_mark_y());
        let n = f.count_where(x, y, x + dev(scale, 36.0), y + dev(scale, 36.0), |p| {
            Frame::near(p, ink, 40)
        });
        assert!(n > 20, "scale {scale}: the mark draws {n} ink pixels");
    }
}

#[test]
fn the_ask_has_one_line_and_two_buttons() {
    for scale in SCALES {
        let h = harness(scale);
        h.comp.set_ask_open(true);
        h.advance(0);
        let c = &h.comp;
        assert!(
            (c.get_ask_box_w() - 360.0).abs() < 0.01,
            "scale {scale}: box width {}",
            c.get_ask_box_w()
        );
        assert!(
            c.get_ask_text_h() < 24.0,
            "scale {scale}: the line wrapped, it is {} high",
            c.get_ask_text_h()
        );
        assert!(c.get_ask_cancel_x() < c.get_ask_download_x());
        assert!((c.get_ask_cancel_y() - c.get_ask_download_y()).abs() < 0.01);
        let right = c.get_ask_download_x() + c.get_ask_download_w();
        assert!(
            (right - (c.get_ask_box_x() + c.get_ask_box_w() - 20.0 - 1.0 / scale)).abs() <= 1.0,
            "scale {scale}: Download ends at {right}, the box at {}",
            c.get_ask_box_x() + c.get_ask_box_w()
        );
        // Download first: a click says 1, Cancel says 0.
        h.click(c.get_ask_download_x() + 4.0, c.get_ask_download_y() + 4.0);
        assert_eq!(log(&h).trim(), "ask:1");
        h.click(c.get_ask_cancel_x() + 4.0, c.get_ask_cancel_y() + 4.0);
        assert_eq!(log(&h).trim(), "ask:1 ask:0");
        // A press outside the box is Cancel too.
        h.click(4.0, 4.0);
        assert_eq!(log(&h).trim(), "ask:1 ask:0 ask:0");
        // Escape is Cancel.
        h.key_text(&String::from(char::from(slint::platform::Key::Escape)));
        assert_eq!(log(&h).trim(), "ask:1 ask:0 ask:0 ask:0", "scale {scale}");
        // The question's own words are the only text: the line and two button words.
        assert_eq!(c.get_ask_text(), LINE);
    }
}

#[test]
fn the_key_field_waits_in_accent_and_offers_reset_only_when_idle_and_not_default() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        assert!(c.get_key_w() >= 112.0 - 0.01, "scale {scale}: min width");
        assert!((c.get_key_h() - 32.0).abs() < 0.01);
        assert!(!c.get_reset_shown(), "default key: no Reset");
        c.set_key_reset(true);
        h.advance(0);
        assert!(c.get_reset_shown(), "a changed key offers Reset");
        c.set_capturing(true);
        h.advance(0);
        assert!(!c.get_reset_shown(), "no Reset while a key is awaited");
        // The edge is accent-hi while it waits.
        let hi = c.global::<Theme>().get_accent_hi();
        let want = [hi.red(), hi.green(), hi.blue(), 255];
        let f = h.render();
        let y = dev(scale, c.get_key_y() + 16.0);
        let x = dev(scale, c.get_key_x());
        assert!(
            Frame::near(f.px(x, y), want, 3) || Frame::near(f.px(x + 1, y), want, 3),
            "scale {scale}: the capture edge is accent-hi, got {:?}",
            f.px(x, y)
        );
        // A press on the field says so.
        c.set_capturing(false);
        h.advance(0);
        h.click(c.get_key_x() + 8.0, c.get_key_y() + 8.0);
        assert_eq!(log(&h).trim(), "key");
    }
}

#[test]
fn the_mic_row_draws_its_track_idle_and_fills_it_while_testing() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        assert!((c.get_track_w() - 96.0).abs() < 0.01 && (c.get_track_h() - 6.0).abs() < 0.01);
        let track = c.global::<Theme>().get_track();
        let track = [track.red(), track.green(), track.blue(), 255];
        let drawn = |f: &Frame| {
            f.count_where(
                dev(scale, c.get_track_x()),
                dev(scale, c.get_track_y()),
                dev(scale, c.get_track_x() + 96.0),
                dev(scale, c.get_track_y() + 6.0),
                |p| Frame::near(p, track, 3),
            )
        };
        let idle = drawn(&h.render());
        assert!(idle > 0, "scale {scale}: the idle track is drawn");
        assert_eq!(c.get_fill_w(), 0.0);
        c.set_mic_testing(true);
        c.set_mic_level(0.5);
        h.advance(200);
        assert!(
            (c.get_fill_w() - 48.0).abs() < 0.5,
            "scale {scale}: fill {}",
            c.get_fill_w()
        );
        assert!(drawn(&h.render()) < idle, "the fill covers the track");
        // The track never moves between idle and testing; Stop reads instead of Test.
        c.set_mic_level(3.0);
        h.advance(200);
        assert!(
            (c.get_fill_w() - 96.0).abs() < 0.5,
            "the meter stops at full"
        );
        h.click(c.get_test_x() + 6.0, c.get_track_y() - 6.0);
        assert_eq!(log(&h).trim(), "test");
    }
}

#[test]
fn the_limited_mark_is_24_square_and_says_the_line_on_hover() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        c.set_limited(true);
        h.advance(0);
        assert!((c.get_mark_w() - 24.0).abs() < 0.01 && (c.get_mark_h() - 24.0).abs() < 0.01);
        h.move_to(c.get_mark_x() + 12.0, c.get_mark_y() + 12.0);
        h.advance(0);
        assert_eq!(c.get_tips(), 1, "scale {scale}: one tip on hover");
        assert_eq!(c.get_tip_text(), LINE);
    }
}

#[test]
fn model_buttons_press_by_id_and_a_dimmed_row_presses_nothing() {
    let h = harness(2.0);
    let c = &h.comp;
    let first = |y: f32| (c.get_panel_x() + 1.0 + c.get_a_right_x() + 4.0, y + 32.0);
    let (x, y) = first(c.get_a_y());
    h.click(x, y);
    assert_eq!(log(&h).trim(), "a:uninstall");
    // Row c: Download.
    let (x, y) = (
        c.get_panel_x() + 1.0 + c.get_c_right_x() + 4.0,
        c.get_c_y() + 32.0,
    );
    h.click(x, y);
    assert_eq!(log(&h).trim(), "a:uninstall c:download");
    // A dimmed row (dictation off): its buttons do nothing.
    let mut dim = c.get_row_c();
    dim.off = true;
    c.set_row_c(dim);
    h.advance(0);
    h.click(x, y);
    assert_eq!(
        log(&h).trim(),
        "a:uninstall c:download",
        "an off row ignores a press"
    );
}

#[test]
fn rows_hold_at_every_scale() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        // The microphone row: track, Test, picker, in that order, none over another, inside 600.
        assert!(c.get_track_x() + c.get_track_w() <= c.get_test_x() + 0.01);
        assert!(c.get_test_x() < c.get_select_x());
        assert!(c.get_mic_right() <= 20.0 + 600.0 + 0.01, "scale {scale}");
        assert!(c.get_select_w() >= 168.0 - 0.01);
        // Each model row: the text column ends before the controls, the controls end 16 from the edge.
        let inner = c.get_panel_w() - 2.0 * (1.0 / scale);
        for (what, text_right, right_x) in [
            ("in use", c.get_a_text_right_x(), c.get_a_right_x()),
            ("downloading", c.get_b_text_right_x(), c.get_b_right_x()),
            ("not downloaded", c.get_c_text_right_x(), c.get_c_right_x()),
        ] {
            assert!(
                text_right <= right_x + 0.01,
                "scale {scale}: {what}: the text column ends at {text_right}, the controls start at {right_x}"
            );
            assert!(
                right_x > 64.0,
                "scale {scale}: {what}: controls clear the mark"
            );
            assert!(
                right_x <= inner - 16.0 - 40.0,
                "scale {scale}: {what}: controls inside the row"
            );
        }
        // Nothing is drawn past the panel's right edge.
        let f = h.render();
        let panel_right = dev(scale, c.get_panel_x() + c.get_panel_w());
        let ground = f.px(panel_right + 3, dev(scale, c.get_a_y()) + 4);
        let past = f.count_where(
            panel_right + 3,
            dev(scale, c.get_a_y()),
            panel_right + dev(scale, 20.0),
            dev(scale, c.get_c_y() + 64.0),
            |p| !Frame::near(p, ground, 12),
        );
        assert_eq!(past, 0, "scale {scale}: ink past the panel");
    }
}

#[test]
fn a_row_reports_its_look_with_the_mark_and_its_buttons() {
    for scale in SCALES {
        let h = harness(scale);
        let c = &h.comp;
        let (a, rc) = (c.get_look_a(), c.get_look_c());
        assert_eq!(a.id.as_str(), "base", "scale {scale}: the row names itself");
        assert!(
            a.mark_x < a.name_x && (a.mark_w - 36.0).abs() < 0.01,
            "scale {scale}: mark {} to {} wide, name {}",
            a.mark_x,
            a.mark_w,
            a.name_x
        );
        assert!(
            (a.mark_y + a.mark_h / 2.0 - a.row_h / 2.0).abs() <= 1.0 / scale + 0.01,
            "scale {scale}: the mark is centred on the row"
        );
        assert_eq!(a.b0_id.as_str(), "uninstall");
        assert_eq!(rc.b0_id.as_str(), "download");
        let text = c.global::<Theme>().get_text();
        assert_eq!(a.b0_ink, text, "scale {scale}: the ink");
        assert_eq!(
            a.b0_fill, rc.b0_fill,
            "scale {scale}: one fill for every button"
        );
        assert_eq!(a.b0_edge, rc.b0_edge, "scale {scale}: one border");
        assert_eq!(a.b0_radius, rc.b0_radius, "scale {scale}: one radius");
        assert!(a.b0_radius > 0.0 && a.b0_w > 0.0 && a.b0_h > 0.0);
        // The button sits where the row's controls begin, so a click can be aimed at it.
        assert!(
            a.b0_x >= c.get_a_right_x() - 0.01,
            "scale {scale}: b0 x {}",
            a.b0_x
        );
        assert_eq!(a.b1_id.as_str(), "", "scale {scale}: no second button");
    }
}
