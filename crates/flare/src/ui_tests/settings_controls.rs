//! UI tests of the settings controls (M7 Task 31); headless only.
//!
//! Every test runs on the headless harness (no window). Box geometry comes from the preview's out
//! properties (logical px); colours are read from the rendered frame and compared to Theme's tokens.

use crate::ControlsPreview;
use crate::ui_harness::{Frame, Harness, SCALES};

/// `Theme.accent` and `Theme.accent-hi`, `Theme.sel-bg`, `Theme.track`, `Theme.switch-knob`,
/// `SettingsFixed.white`, `Theme.pressed` and `Theme.control` (the pt-default values).
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];
const TRACK: [u8; 4] = [0x5e, 0x5c, 0x59, 255];
const KNOB_DARK: [u8; 4] = [0x0b, 0x0b, 0x0f, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const PRESSED: [u8; 4] = [0x3d, 0x3c, 0x39, 255];
const CONTROL: [u8; 4] = [0x1e, 0x1e, 0x1d, 255];

const CSS_W: u32 = 480;
const CSS_H: u32 = 200;

fn harness(scale: f32) -> Harness<ControlsPreview> {
    Harness::new(scale, CSS_W, CSS_H, || ControlsPreview::new().unwrap())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn is_accent(p: [u8; 4]) -> bool {
    Frame::near(p, ACCENT, 4) || Frame::near(p, ACCENT_HI, 4)
}

/// The rectangle of an out-property box: (x, y, w, h), logical px.
fn sw_rect(h: &Harness<ControlsPreview>) -> (f32, f32, f32, f32) {
    (
        h.comp.get_sw_x(),
        h.comp.get_sw_y(),
        h.comp.get_sw_w(),
        h.comp.get_sw_h(),
    )
}

fn save_rect(h: &Harness<ControlsPreview>) -> (f32, f32, f32, f32) {
    (
        h.comp.get_save_x(),
        h.comp.get_save_y(),
        h.comp.get_save_w(),
        h.comp.get_save_h(),
    )
}

fn seg_rect(h: &Harness<ControlsPreview>) -> (f32, f32, f32, f32) {
    (
        h.comp.get_seg_x(),
        h.comp.get_seg_y(),
        h.comp.get_seg_w(),
        h.comp.get_seg_h(),
    )
}

fn reset_rect(h: &Harness<ControlsPreview>) -> (f32, f32, f32, f32) {
    (
        h.comp.get_reset_x(),
        h.comp.get_reset_y(),
        h.comp.get_reset_w(),
        h.comp.get_reset_h(),
    )
}

fn centre((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// The rendered pixel at a logical point.
fn px_at(h: &Harness<ControlsPreview>, frame: &Frame, x: f32, y: f32) -> [u8; 4] {
    frame.px(h.phys(x), h.phys(y))
}

/// Whether the physical pixel lies in a logical box, with a 2 px allowance for anti-aliasing.
fn inside(h: &Harness<ControlsPreview>, px: u32, py: u32, r: (f32, f32, f32, f32)) -> bool {
    let (x, y, w, hh) = r;
    let (x0, y0) = (h.phys(x) as i64, h.phys(y) as i64);
    let (x1, y1) = (h.phys(x + w) as i64, h.phys(y + hh) as i64);
    let (px, py) = (px as i64, py as i64);
    px + 2 >= x0 && px <= x1 + 2 && py + 2 >= y0 && py <= y1 + 2
}

#[test]
fn switch_is_36_by_20_with_a_16_knob_at_every_scale() {
    for s in SCALES {
        let h = harness(s);
        assert!(close(h.comp.get_sw_w(), 36.0), "width at {s}");
        assert!(close(h.comp.get_sw_h(), 20.0), "height at {s}");
        assert!(close(h.comp.get_knob_w(), 16.0), "knob at {s}");
        assert!(close(h.comp.get_knob_x(), 18.0), "on knob at {s}");
        h.comp.set_sw_on(false);
        assert!(close(h.comp.get_knob_x(), 2.0), "off knob at {s}");
    }
}

#[test]
fn an_on_switch_wears_the_accent_fill_and_the_theme_knob() {
    for s in SCALES {
        let h = harness(s);
        h.advance(300);
        let (x, y, _, _) = sw_rect(&h);
        let f = h.render();
        assert!(
            Frame::near(px_at(&h, &f, x + 8.0, y + 10.0), ACCENT, 2),
            "on track at {s}"
        );
        assert!(
            Frame::near(px_at(&h, &f, x + 26.0, y + 10.0), KNOB_DARK, 2),
            "on knob at {s}"
        );

        h.comp.set_sw_on(false);
        h.advance(300);
        let f = h.render();
        assert!(
            Frame::near(px_at(&h, &f, x + 30.0, y + 10.0), TRACK, 2),
            "off track at {s}"
        );
        assert!(
            Frame::near(px_at(&h, &f, x + 10.0, y + 10.0), WHITE, 2),
            "off knob at {s}"
        );
    }
}

#[test]
fn a_disabled_switch_is_dimmed_and_does_not_toggle() {
    let h = harness(1.0);
    h.comp.set_sw_disabled(true);
    h.advance(300);
    let c = centre(sw_rect(&h));
    h.click(c.0, c.1);
    assert_eq!(
        h.comp.get_toggles(),
        0,
        "a click on a disabled switch does nothing"
    );
    let (x, y, _, _) = sw_rect(&h);
    let f = h.render();
    assert!(
        !Frame::near(px_at(&h, &f, x + 8.0, y + 10.0), ACCENT, 2),
        "a disabled switch is dimmed, not the full accent"
    );
}

#[test]
fn a_click_flips_a_switch_once() {
    let h = harness(1.0);
    h.advance(300);
    let c = centre(sw_rect(&h));
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_toggles(), 1);
    assert!(!h.comp.get_sw_on());
}

#[test]
fn space_and_enter_toggle_a_focused_switch() {
    let h = harness(1.0);
    h.advance(300);
    h.comp.invoke_focus_switch();
    h.key_text(" ");
    assert!(!h.comp.get_sw_on(), "Space turns the switch off");
    h.key_text("\n");
    assert!(h.comp.get_sw_on(), "Enter turns it back on");
    assert_eq!(h.comp.get_toggles(), 2);
}

#[test]
fn the_pressed_segment_is_one_step_off_the_control() {
    let h = harness(1.0);
    h.advance(300);
    let f = h.render();
    let (x, y, w, hh) = seg_rect(&h);
    let (x0, y0) = (h.phys(x), h.phys(y));
    let (x1, y1) = (h.phys(x + w), h.phys(y + hh));
    let pressed = f.count_where(x0, y0, x1, y1, |p| Frame::near(p, PRESSED, 2));
    let control = f.count_where(x0, y0, x1, y1, |p| Frame::near(p, CONTROL, 2));
    let accent = f.count_where(x0, y0, x1, y1, is_accent);
    assert!(pressed > 0, "the chosen segment is filled Theme.pressed");
    assert!(control > 0, "the pill is filled Theme.control");
    assert_eq!(accent, 0, "a segment never wears the accent");
}

#[test]
fn a_segment_click_chooses_it() {
    let h = harness(1.0);
    h.advance(300);
    let (x, y, w, hh) = seg_rect(&h);
    // the right-hand padding of "Light" (the second segment)
    h.click(x + w - 8.0, y + hh / 2.0);
    assert_eq!(h.comp.get_seg_value().as_str(), "light");
    assert_eq!(h.comp.get_seg_picks(), 1);
}

#[test]
fn save_is_accent_when_dirty_and_grey_when_clean() {
    let h = harness(1.0);
    h.advance(300);
    let (x, y, _, hh) = save_rect(&h);
    let f = h.render();
    assert!(
        Frame::near(px_at(&h, &f, x + 3.0, y + hh / 2.0), ACCENT, 3),
        "dirty Save is the accent"
    );
    let c = centre(save_rect(&h));
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_save_clicks(), 1, "a dirty Save fires");

    h.comp.set_save_dirty(false);
    h.advance(300);
    let f = h.render();
    let p = px_at(&h, &f, x + 3.0, y + hh / 2.0);
    assert!(!is_accent(p), "clean Save is not the accent");
    assert!(
        p[0].abs_diff(p[1]) < 8 && p[1].abs_diff(p[2]) < 8,
        "clean Save is grey"
    );
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_save_clicks(), 1, "a clean Save fires nothing");
}

#[test]
fn row_button_is_32_high_and_the_marked_button_keeps_its_width() {
    let h = harness(1.0);
    assert!(close(h.comp.get_btn_h(), 32.0));
    let plain = h.comp.get_btn_w();
    h.comp.set_btn_alt(true);
    h.advance(300);
    assert!(
        close(h.comp.get_btn_w(), plain),
        "Mark and Marked share one width"
    );
}

#[test]
fn a_row_button_press_and_a_keyboard_press_both_fire() {
    let h = harness(1.0);
    let (bx, by) = (
        200.0 + h.comp.get_btn_w() / 2.0,
        130.0 + h.comp.get_btn_h() / 2.0,
    );
    h.click(bx, by);
    assert_eq!(h.comp.get_btn_clicks(), 1);
    h.comp.invoke_focus_button();
    h.key_text(" ");
    assert_eq!(h.comp.get_btn_clicks(), 2);
}

#[test]
fn reset_shows_its_underline_only_on_hover() {
    let h = harness(1.0);
    h.advance(300);
    let (x, y, w, hh) = reset_rect(&h);
    let band = |f: &Frame| {
        let y1 = h.phys(y + hh);
        f.count_where(h.phys(x), y1 - 1, h.phys(x + w), y1, |p| {
            Frame::near(p, ACCENT_HI, 4)
        })
    };
    let f = h.render();
    assert_eq!(band(&f), 0, "no underline without hover");
    let c = centre((x, y, w, hh));
    h.move_to(c.0, c.1);
    h.advance(50);
    let f = h.render();
    assert!(band(&f) > 0, "the underline shows on hover");
}

#[test]
fn reset_key_press_fires_it() {
    let h = harness(1.0);
    h.comp.invoke_focus_reset();
    h.key_text("\n");
    assert_eq!(h.comp.get_reset_clicks(), 1);
}

#[test]
fn no_control_but_save_and_the_on_switch_paints_the_accent() {
    for s in [1.0, 2.25] {
        let h = harness(s);
        h.advance(300);
        let f = h.render();
        let allowed = [sw_rect(&h), save_rect(&h), reset_rect(&h)];
        let stray = f.count_where(0, 0, f.w, f.h, is_accent);
        let mut outside = 0;
        for py in 0..f.h {
            for px in 0..f.w {
                if is_accent(f.px(px, py)) && !allowed.iter().any(|r| inside(&h, px, py, *r)) {
                    outside += 1;
                }
            }
        }
        assert!(
            stray > 0,
            "the on switch, Save and Reset paint the accent at {s}"
        );
        assert_eq!(outside, 0, "no other control paints the accent at {s}");
    }
}

/// Prism Terminal's "Panel edges" pill (None, Faint, Hairline, Solid) is 3.0, 2.0 and 1.8 css px
/// narrower than the 233 Flare drew, at 100, 150 and 225 percent (the pill's left edge on the same
/// row of 05-settings-appearance, the right edges being level): 230.0, 231.0 and 231.2 wide, because
/// Chromium lays each label out at its own fractional width. Slint rounds a Text's preferred width UP
/// to a whole css px, so the pill came out 233 and its first label sat 2 to 3 css px left of PT's
/// (audit V13, 2026-10-21).
const PT_PANEL_EDGES_PILL_W: [(f32, f32); 3] = [(1.0, 230.0), (1.5, 231.0), (2.25, 231.2)];

#[cfg_attr(
    not(windows),
    ignore = "PT's widths are Segoe UI's (Windows); the macOS twin is a_segmented_pill_keeps_its_fractional_width_on_macos"
)]
#[test]
fn a_segmented_pill_is_as_wide_as_pts_not_a_whole_pixel_per_label_wider() {
    pill_widths_hold(&PT_PANEL_EDGES_PILL_W, "PT's");
}

/// The same pill in macOS's Flare Selawik (rule 1232), measured on the port (2026-11-13): there is no PT
/// reference on a Mac, so these are Flare's own widths. Selawik's letters run about 1 percent wider than
/// Segoe UI's, so the pill is 2 to 3 css px wider than PT's. They are fractional, like PT's: a label
/// rounded up to a whole css px again would make the pill 2 to 3 px wider still and fail this.
#[cfg(not(windows))]
const MAC_PANEL_EDGES_PILL_W: [(f32, f32); 3] = [(1.0, 232.94), (1.5, 232.27), (2.25, 232.72)];

/// macOS twin of `a_segmented_pill_is_as_wide_as_pts_not_a_whole_pixel_per_label_wider`.
#[cfg(not(windows))]
#[test]
fn a_segmented_pill_keeps_its_fractional_width_on_macos() {
    pill_widths_hold(&MAC_PANEL_EDGES_PILL_W, "the macOS reference");
    for (s, _) in MAC_PANEL_EDGES_PILL_W {
        let h = harness(s);
        h.advance(0);
        let w = h.comp.get_seg4_w();
        assert!(w.fract() != 0.0, "the pill is a whole {w} css px at {s}");
    }
}

fn pill_widths_hold(widths: &[(f32, f32)], whose: &str) {
    for &(s, want) in widths {
        let h = harness(s);
        h.advance(0);
        let w = h.comp.get_seg4_w();
        assert!(
            (w - want).abs() <= 1.4,
            "the four-label pill is {w} css px at {s}, {whose} is {want}"
        );
    }
}
