//! component and flow tests for the dialog (Tasks 24 and 31).
//!
//! Every test runs on the headless harness (no window). Button and box geometry come from the
//! preview's out properties; colours are read from the rendered frame.

use slint::StyledText;

use crate::DialogPreview;
use crate::ui_harness::{Frame, Harness, SCALES};
use flare_agent::kinds::DetectedAgent;
use flare_tabs::close_flow::{NAME_INK, close_question_body};

/// `Theme.accent`, `Theme.danger-fill`, `Theme.text` and `Theme.dim`.
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];
const DANGER: [u8; 4] = [0xb4, 0x35, 0x3f, 255];
const INK: [u8; 4] = [0xde, 0xda, 0xcf, 255];
const DIM: [u8; 4] = [0x92, 0x90, 0x89, 255];

/// Relative brightness of a pixel, used to tell the name ink from the dim text.
fn lum(p: [u8; 4]) -> u32 {
    (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000
}

fn harness(scale: f32, w: u32, h: u32) -> Harness<DialogPreview> {
    let hn = Harness::new(scale, w, h, || DialogPreview::new().unwrap());
    hn.comp
        .set_dialog_title("Close the tab and end the agent?".into());
    hn.comp.set_primary_label("Close tab".into());
    hn.comp.set_cancel_label("Cancel".into());
    hn
}

fn open(h: &Harness<DialogPreview>) {
    h.comp.set_open(true);
    h.advance(0);
}

/// The centre of a box, in CSS pixels.
fn centre(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

fn primary_centre(h: &Harness<DialogPreview>) -> (f32, f32) {
    centre(
        h.comp.get_primary_x(),
        h.comp.get_primary_y(),
        h.comp.get_primary_w(),
        h.comp.get_primary_h(),
    )
}

fn cancel_centre(h: &Harness<DialogPreview>) -> (f32, f32) {
    centre(
        h.comp.get_cancel_x(),
        h.comp.get_cancel_y(),
        h.comp.get_cancel_w(),
        h.comp.get_cancel_h(),
    )
}

#[test]
fn enter_confirms_because_the_primary_has_the_focus() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_text("\n");
    assert_eq!(h.comp.get_confirms(), 1);
    assert_eq!(h.comp.get_cancels(), 0);
}

#[test]
fn escape_cancels_and_is_swallowed() {
    let h = harness(1.0, 1000, 600);
    h.comp.invoke_focus_terminal();
    open(&h);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(
        h.comp.get_term_key().as_str(),
        "",
        "the window behind got no key"
    );
}

#[test]
fn escape_does_nothing_while_locked_and_is_still_swallowed() {
    let h = harness(1.0, 1000, 600);
    h.comp.invoke_focus_terminal();
    h.comp.set_locked(true);
    open(&h);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_cancels(), 0);
    assert_eq!(
        h.comp.get_term_key().as_str(),
        "",
        "the window behind got no key"
    );
}

/// The app-link question (rule 252): Cancel is the default, so Enter, Space, Escape and a press outside all cancel, and
/// Open needs a click or Tab then Enter.
#[test]
fn with_cancel_the_default_enter_space_escape_and_a_press_outside_cancel() {
    for keys in ["\n", " ", "\u{1b}", "\t "] {
        let h = harness(1.0, 1000, 600);
        h.comp.set_default_cancel(true);
        open(&h);
        h.key_text(keys);
        assert_eq!(h.comp.get_cancels(), 1, "{keys:?} cancels");
        assert_eq!(h.comp.get_confirms(), 0, "{keys:?} never opens");
    }
    let h = harness(1.0, 1000, 600);
    h.comp.set_default_cancel(true);
    open(&h);
    h.click(5.0, 5.0);
    assert_eq!((h.comp.get_cancels(), h.comp.get_confirms()), (1, 0));
}

#[test]
fn with_cancel_the_default_tab_then_enter_or_a_click_confirms() {
    let h = harness(1.0, 1000, 600);
    h.comp.set_default_cancel(true);
    open(&h);
    h.key_text("\t\n");
    assert_eq!((h.comp.get_cancels(), h.comp.get_confirms()), (0, 1));
    let h = harness(1.0, 1000, 600);
    h.comp.set_default_cancel(true);
    open(&h);
    let (px, py) = primary_centre(&h);
    h.click(px, py);
    assert_eq!((h.comp.get_cancels(), h.comp.get_confirms()), (0, 1));
    // Every other question keeps the primary as its default.
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_text(" ");
    assert_eq!((h.comp.get_cancels(), h.comp.get_confirms()), (0, 1));
}

#[test]
fn tab_stays_inside_the_two_buttons() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    // primary -> cancel -> primary -> cancel, then Enter on cancel
    h.key_text("\t\t\t\n");
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_confirms(), 0);
}

#[test]
fn shift_tab_from_the_primary_lands_on_cancel() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_text("\u{19}\n"); // Backtab, then Enter
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_confirms(), 0);
}

#[test]
fn a_press_on_the_scrim_cancels_a_press_in_the_box_does_not() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.click(5.0, 5.0);
    assert_eq!(h.comp.get_cancels(), 1, "the scrim cancels");
    // the middle of the box, over the title and body
    h.click(500.0, 300.0);
    assert_eq!(
        h.comp.get_cancels(),
        1,
        "a press in the box does not cancel"
    );
    assert_eq!(h.comp.get_confirms(), 0);
    assert_eq!(
        h.comp.get_behind_clicks(),
        0,
        "nothing behind the dialog counted a click"
    );
}

#[test]
fn a_press_on_the_scrim_does_nothing_while_locked() {
    let h = harness(1.0, 1000, 600);
    h.comp.set_locked(true);
    open(&h);
    h.click(5.0, 5.0);
    assert_eq!(h.comp.get_cancels(), 0);
}

#[test]
fn the_primary_and_cancel_buttons_report() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    let (px, py) = primary_centre(&h);
    h.click(px, py);
    assert_eq!(h.comp.get_confirms(), 1);
    let (cx, cy) = cancel_centre(&h);
    h.click(cx, cy);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_confirms(), 1);
}

#[test]
fn the_box_is_420_wide_at_most_and_centred() {
    for scale in [1.0_f32, 2.25] {
        let h = harness(scale, 1000, 600);
        open(&h);
        let w = h.comp.get_box_w();
        assert!((w - 420.0).abs() <= 1.0 / scale, "box width {w} at {scale}");
        let mid = h.comp.get_box_x() + w / 2.0;
        assert!(
            (mid - 500.0).abs() <= 1.0 / scale,
            "centred at {scale}: {mid}"
        );
    }
    let narrow = harness(1.0, 300, 600);
    open(&narrow);
    assert!(
        (narrow.comp.get_box_w() - 252.0).abs() <= 0.01,
        "300 wide window"
    );
}

#[test]
fn the_primary_wears_the_accent_or_the_danger_fill() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    let (px, py) = (h.comp.get_primary_x(), h.comp.get_primary_y());
    let ph = h.comp.get_primary_h();
    let sample = |h: &Harness<DialogPreview>| {
        let f = h.render();
        f.px(h.phys(px + 4.0), h.phys(py + ph / 2.0))
    };
    assert!(Frame::near(sample(&h), ACCENT, 3), "accent fill");
    h.comp.set_danger(true);
    h.advance(0);
    assert!(Frame::near(sample(&h), DANGER, 3), "danger fill");
}

/// PT's buttons carry `focus-visible:ring-2`: Chromium shows that ring only after the keyboard, so a
/// dialog opened by a click rests with no ring on the confirm (owner, 2026-10-22).
#[test]
fn the_ring_shows_only_after_the_keyboard() {
    let h = harness(1.0, 1000, 600);
    let (px, py, pw) = (
        h.comp.get_primary_x(),
        h.comp.get_primary_y(),
        h.comp.get_primary_w(),
    );
    open(&h);
    assert!(!h.comp.get_ring_shown(), "no ring at rest after a click");
    let edge = |h: &Harness<DialogPreview>| {
        let f = h.render();
        f.px(h.phys(px + pw / 2.0), h.phys(py - 1.0))
    };
    assert!(
        !Frame::near(edge(&h), ACCENT_HI, 3),
        "no ring outside the confirm at rest"
    );
    h.key_text("\t");
    assert!(h.comp.get_ring_shown(), "Tab brings the ring");
    h.key_text("\t");
    assert!(
        Frame::near(edge(&h), ACCENT_HI, 3),
        "the ring is drawn outside the confirm"
    );
}

/// A PT dialog button is a native button, so Space picks the focused one, as Enter does.
#[test]
fn space_picks_the_focused_button_like_enter() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_text(" ");
    assert_eq!(h.comp.get_confirms(), 1);
}

#[test]
fn a_dialog_opened_by_a_chord_shows_the_ring_from_the_start() {
    let h = harness(1.0, 1000, 600);
    h.comp.set_opened_by_key(true);
    open(&h);
    assert!(h.comp.get_ring_shown());
    h.comp.set_open(false);
    h.comp.set_opened_by_key(false);
    h.advance(0);
    open(&h);
    assert!(
        !h.comp.get_ring_shown(),
        "the next open by a click has none"
    );
}

#[test]
fn the_body_names_wear_the_text_ink() {
    let h = harness(1.0, 1000, 600);
    h.comp.set_dialog_title("".into());
    let body = StyledText::from_markdown(&format!(
        "<font color=\"{NAME_INK}\">Claude</font> is running"
    ))
    .expect("the body parses");
    h.comp.set_body(body);
    open(&h);
    let f = h.render();
    // the name's ink is the only text brighter than the dim body text
    let bright = |p: [u8; 4]| lum(p) >= 200;
    let (x0, y0, x1, y1) = f
        .bbox_where(0, 0, f.w, f.h, bright)
        .expect("the name is drawn");
    let mut name_max = [0u8; 4];
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = f.px(x, y);
            if bright(p) && lum(p) > lum(name_max) {
                name_max = p;
            }
        }
    }
    assert!(Frame::near(name_max, INK, 12), "name ink {name_max:?}");
    let mut rest_max = [0u8; 4];
    for y in y0..=y1 {
        // Past a gap of two device pixels; the glyph edges of the name blend into the band
        // just beyond its last pixel, so those are not the dim text.
        for x in (x1 + 3)..f.w {
            let p = f.px(x, y);
            if lum(p) > lum(rest_max) && lum(p) > 60 && lum(p) < 170 {
                rest_max = p;
            }
        }
    }
    assert!(Frame::near(rest_max, DIM, 12), "dim body {rest_max:?}");
    assert!(
        lum(rest_max) < lum(name_max),
        "no rest pixel as bright as the name"
    );
}

#[test]
fn the_close_bodies_parse_as_styled_text() {
    let bodies = [
        close_question_body(DetectedAgent::Claude, "api", None, 0, false, NAME_INK),
        close_question_body(
            DetectedAgent::Claude,
            "api",
            Some(180_000),
            0,
            false,
            NAME_INK,
        ),
        close_question_body(
            DetectedAgent::Codex,
            "api",
            Some(180_000),
            1,
            false,
            NAME_INK,
        ),
        close_question_body(
            DetectedAgent::Claude,
            "api",
            Some(180_000),
            2,
            true,
            NAME_INK,
        ),
        close_question_body(DetectedAgent::Other, "api", None, 0, false, NAME_INK),
        close_question_body(DetectedAgent::Claude, "a<b>_[x]", None, 0, false, NAME_INK),
    ];
    for b in bodies {
        assert!(
            StyledText::from_markdown(&b).is_ok(),
            "body does not parse: {b}"
        );
    }
}

#[test]
fn only_dialog_slint_declares_a_scrim() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");
    let mut declaring = Vec::new();
    for entry in std::fs::read_dir(dir).expect("ui folder") {
        // sync-ok: test code only, never the app
        let path = entry.expect("entry").path(); // sync-ok: test code only, never the app
        if path.extension().and_then(|e| e.to_str()) != Some("slint") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("slint file"); // sync-ok: test code only, never the app
        if text.contains("modal-scrim") {
            declaring.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("dialog.slint") {
            assert!(
                text.starts_with("// modal-scrim:"),
                "the marker is the first line"
            );
            assert!(text.contains("Key.Tab"), "Tab is kept inside");
            assert!(text.contains("Key.Escape"), "Escape is handled");
            assert!(text.contains("locked"), "the locked property exists");
        }
    }
    assert_eq!(declaring, vec!["dialog.slint".to_string()]);
}

#[test]
fn focus_returns_to_the_terminal_after_close() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    assert!(
        h.comp.get_chrome_has_focus(),
        "the dialog holds the focus while open"
    );
    h.comp.set_open(false);
    h.advance(0);
    h.comp.invoke_focus_terminal();
    assert!(!h.comp.get_chrome_has_focus());
}

#[test]
fn a_new_title_puts_the_focus_back_on_the_primary() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_text("\t"); // now on cancel
    h.comp.set_dialog_title("Stop the agent and close?".into());
    h.advance(0);
    h.key_text("\n");
    assert_eq!(h.comp.get_confirms(), 1);
    assert_eq!(h.comp.get_cancels(), 0);
}

#[test]
fn every_scale_keeps_the_box_centred() {
    for scale in SCALES {
        let h = harness(scale, 1000, 600);
        open(&h);
        let mid = h.comp.get_box_x() + h.comp.get_box_w() / 2.0;
        assert!((mid - 500.0).abs() <= 1.0 / scale, "scale {scale}");
    }
}

#[test]
fn ctrl_tab_passes_through_the_modal_and_is_not_trapped() {
    let h = harness(1.0, 1000, 600);
    open(&h);
    // dialog.slint rejects a Tab with Ctrl held (the host's chord), so the focus stays on the
    // primary button; a trapped Tab would have moved it to Cancel.
    h.key_with(false, true, "\t");
    h.key_text("\n");
    assert_eq!(h.comp.get_confirms(), 1, "the focus never moved");
    assert_eq!(h.comp.get_cancels(), 0);
    // a plain Tab still is trapped
    let h = harness(1.0, 1000, 600);
    open(&h);
    h.key_with(false, false, "\t");
    h.key_text("\n");
    assert_eq!(h.comp.get_cancels(), 1);
}

#[test]
fn a_press_outside_lifts_the_drag_region_at_every_scale() {
    // The title row's WindowMoveArea sits under the dialog; the scrim covers the whole window, so
    // a press in the title band hits the scrim (cancels) and never starts a window move.
    for scale in SCALES {
        let h = harness(scale, 1000, 600);
        open(&h);
        h.click(500.0, 4.0);
        assert_eq!(
            h.comp.get_cancels(),
            1,
            "title band press cancels at {scale}"
        );
        assert_eq!(
            h.comp.get_behind_clicks(),
            0,
            "nothing behind it at {scale}"
        );
    }
    let app = include_str!("../../ui/app.slint").replace('\r', "");
    let dlg = app
        .find("dlg := Dialog {")
        .expect("the dialog in app.slint");
    let last_move = app.rfind("WindowMoveArea").expect("a move area");
    assert!(
        last_move < dlg,
        "every drag region is declared below the dialog"
    );
    let tail = &app[dlg..];
    assert!(
        tail.contains("width: root.width;") && tail.contains("height: root.height;"),
        "the dialog covers the whole window"
    );
    let dialog = include_str!("../../ui/dialog.slint").replace('\r', "");
    assert!(
        dialog.contains(
            "scrim := TouchArea {\n        width: parent.width;\n        height: parent.height;"
        ),
        "the scrim fills the dialog"
    );
}
