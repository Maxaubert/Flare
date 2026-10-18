//! M9 Task 25: the update window on the headless harness (Task 33 appends the eight-scale tests, and
//! Task 34 the flows). Geometry comes from the preview's out properties (css pixels); colours are read
//! from the rendered frame.

use std::rc::Rc;

use slint::{ModelRc, SharedString, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{NoteSection, UpdateWindowPreview};

/// `Theme.ground` (the backdrop), `Theme.side-flat` (the box), `Theme.accent-hi` (the dot),
/// `Theme.accent-solid` (the fill).
const GROUND: [u8; 4] = [0x17, 0x17, 0x17, 255];
const SIDE_FLAT: [u8; 4] = [0x1f, 0x1f, 0x1e, 255];
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];
const ACCENT_SOLID: [u8; 4] = [0xfe, 0x8f, 0x34, 255];

fn section(heading: &str, entries: &[&str]) -> NoteSection {
    NoteSection {
        heading: heading.into(),
        entries: ModelRc::from(Rc::new(VecModel::from(
            entries
                .iter()
                .map(|e| SharedString::from(*e))
                .collect::<Vec<_>>(),
        ))),
    }
}

fn set_sections(h: &Harness<UpdateWindowPreview>, s: Vec<NoteSection>) {
    h.comp
        .set_sections(ModelRc::from(Rc::new(VecModel::from(s))));
    settle(h);
}

/// Lets the repeaters build and the layout run (the harness lays out when it draws).
fn settle(h: &Harness<UpdateWindowPreview>) {
    h.advance(0);
    let _ = h.render();
    let _ = h.render();
}

fn harness_sized(scale: f32, w: u32, hh: u32) -> Harness<UpdateWindowPreview> {
    let h = Harness::new(scale, w, hh, || {
        UpdateWindowPreview::new().expect("the preview builds")
    });
    h.comp.set_window_title("Update to 0.56.0".into());
    h.comp.set_current_text("You have 0.55.6".into());
    set_sections(
        &h,
        vec![
            section(
                "New features",
                &["Open in Flare from the Explorer", "A faster start"],
            ),
            section("Bug fixes", &["The tab strip no longer jumps"]),
        ],
    );
    h.comp.set_notes_max_h(0.44 * hh as f32);
    h.advance(0);
    h
}

fn harness(scale: f32) -> Harness<UpdateWindowPreview> {
    harness_sized(scale, 1000, 640)
}

fn open(h: &Harness<UpdateWindowPreview>) {
    h.comp.set_open(true);
    settle(h);
}

fn centre(x: f32, y: f32, w: f32, hh: f32) -> (f32, f32) {
    (x + w / 2.0, y + hh / 2.0)
}

fn install_centre(h: &Harness<UpdateWindowPreview>) -> (f32, f32) {
    centre(
        h.comp.get_install_x(),
        h.comp.get_install_y(),
        h.comp.get_install_w(),
        h.comp.get_install_h(),
    )
}

fn cancel_centre(h: &Harness<UpdateWindowPreview>) -> (f32, f32) {
    centre(
        h.comp.get_cancel_x(),
        h.comp.get_cancel_y(),
        h.comp.get_cancel_w(),
        h.comp.get_cancel_h(),
    )
}

fn set_running(h: &Harness<UpdateWindowPreview>, pct: i32) {
    h.comp.set_running(true);
    h.comp.set_active(true);
    h.comp.set_pct(pct);
    h.comp.set_pct_text(format!("{pct}%").into());
    h.comp.set_status("Downloading the update".into());
    h.comp.set_install_enabled(false);
    h.advance(400);
    settle(h);
}

#[test]
fn closed_it_draws_nothing_and_takes_no_input() {
    let h = harness(1.0);
    let f = h.render();
    for (x, y) in [(5, 5), (500, 320), (995, 635), (300, 200), (700, 450)] {
        assert!(
            Frame::near(f.px(x, y), GROUND, 1),
            "{x},{y}: {:?}",
            f.px(x, y)
        );
    }
    h.click(500.0, 320.0);
    assert_eq!(h.comp.get_behind_clicks(), 1);
    assert_eq!(h.comp.get_cancels(), 0);
}

#[test]
fn the_box_is_460_wide_centred_and_24_from_a_small_window() {
    let h = harness(1.0);
    open(&h);
    assert_eq!(h.comp.get_box_w(), 460.0);
    assert_eq!(h.comp.get_box_x(), 270.0);
    let small = harness_sized(1.0, 400, 640);
    open(&small);
    assert_eq!(small.comp.get_box_w(), 352.0);
    assert_eq!(small.comp.get_box_x(), 24.0);
}

#[test]
fn title_and_current_line_read_as_given() {
    let h = harness(1.0);
    open(&h);
    assert_eq!(h.comp.get_window_title().as_str(), "Update to 0.56.0");
    assert_eq!(h.comp.get_current_text().as_str(), "You have 0.55.6");
    // The title is drawn: ink in the text colour on the box's first text row.
    let f = h.render();
    let x0 = h.comp.get_box_x() as u32 + 20;
    let y0 = h.comp.get_box_y() as u32 + 20;
    let n = f.count_where(x0, y0, x0 + 200, y0 + 20, |p| {
        !Frame::near(p, SIDE_FLAT, 40)
    });
    assert!(n > 30, "{n}");
}

#[test]
fn focus_lands_on_install() {
    let h = harness(1.0);
    h.comp.invoke_focus_terminal();
    open(&h);
    assert_eq!(h.comp.get_focus_idx(), 1);
    assert!(h.comp.get_chrome_has_focus());
    h.key_text("\n");
    assert_eq!(h.comp.get_installs(), 1);
}

#[test]
fn escape_cancels_only_when_idle_and_is_always_swallowed() {
    let h = harness(1.0);
    h.comp.invoke_focus_terminal();
    open(&h);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_term_keys(), 0);
    set_running(&h, 10);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_cancels(), 1, "no cancel while running");
    assert_eq!(
        h.comp.get_term_keys(),
        0,
        "the key never reached the terminal"
    );
    assert!(h.comp.get_chrome_has_focus(), "the box keeps the focus");
}

#[test]
fn a_press_outside_cancels_only_when_idle() {
    let h = harness(1.0);
    open(&h);
    h.click(5.0, 5.0);
    assert_eq!(h.comp.get_cancels(), 1);
    // A press inside the box does nothing.
    h.click(h.comp.get_box_x() + 230.0, h.comp.get_box_y() + 30.0);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_behind_clicks(), 0);
    set_running(&h, 5);
    h.click(5.0, 5.0);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_behind_clicks(), 0);
}

#[test]
fn tab_stays_inside_and_skips_a_disabled_button() {
    let h = harness(1.0);
    h.comp.invoke_focus_terminal();
    open(&h);
    let mut seen = vec![];
    for _ in 0..3 {
        h.key_text("\t");
        seen.push(h.comp.get_focus_idx());
    }
    assert_eq!(seen, vec![0, 1, 0]);
    assert_eq!(h.comp.get_term_keys(), 0);
    h.key_with(true, false, "\t");
    assert_eq!(h.comp.get_focus_idx(), 1);

    let d = harness(1.0);
    d.comp.set_install_enabled(false);
    open(&d);
    assert_eq!(d.comp.get_focus_idx(), 0);
    let mut seen = vec![];
    for _ in 0..3 {
        d.key_text("\t");
        seen.push(d.comp.get_focus_idx());
    }
    assert_eq!(seen, vec![0, 0, 0]);
}

#[test]
fn return_presses_the_focused_button() {
    let h = harness(1.0);
    open(&h);
    h.key_text("\t");
    h.key_text("\n");
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_installs(), 0);
    h.key_text("\t");
    h.key_text("\n");
    assert_eq!(h.comp.get_installs(), 1);
}

#[test]
fn the_notes_list_scrolls_inside_its_cap() {
    let h = harness(1.0);
    open(&h);
    let short_box = h.comp.get_box_h();
    let entries: Vec<String> = (0..40).map(|i| format!("Entry number {i}")).collect();
    let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
    set_sections(&h, vec![section("Bug fixes", &refs)]);
    settle(&h);
    let cap = 0.44 * 640.0;
    assert!(
        h.comp.get_notes_content_h() > cap,
        "{}",
        h.comp.get_notes_content_h()
    );
    assert!(
        (h.comp.get_notes_h() - cap).abs() < 0.01,
        "{}",
        h.comp.get_notes_h()
    );
    assert!(h.comp.get_box_h() < 640.0);
    assert!(h.comp.get_box_h() > short_box);
}

#[test]
fn the_progress_block_never_changes_the_box_size() {
    let h = harness(1.0);
    open(&h);
    let base = h.comp.get_box_h();
    h.comp.set_active(true);
    h.comp.set_status("Update failed".into());
    h.advance(300);
    assert_eq!(h.comp.get_box_h(), base);
    for pct in [0, 50, 100] {
        set_running(&h, pct);
        assert_eq!(h.comp.get_box_h(), base, "pct {pct}");
        h.comp
            .set_status("Installing. The app will close and reopen.".into());
        h.advance(25);
        assert_eq!(h.comp.get_box_h(), base, "pct {pct} installing");
    }
}

#[test]
fn the_fill_is_zero_at_zero_and_the_track_at_100() {
    let h = harness(1.0);
    open(&h);
    set_running(&h, 0);
    assert_eq!(h.comp.get_fill_w(), 0.0);
    set_running(&h, 100);
    assert_eq!(h.comp.get_fill_w(), h.comp.get_track_w());
    assert_eq!(h.comp.get_track_w(), 460.0 - 40.0);
}

#[test]
fn the_fill_ends_on_a_device_pixel_at_all_eight_scales() {
    for s in SCALES {
        let h = harness(s);
        open(&h);
        h.comp.set_running(true);
        h.comp.set_active(true);
        for pct in (0..=100).step_by(7) {
            h.comp.set_pct(pct);
            h.advance(400);
            let dev = h.comp.get_fill_w() * s;
            assert!(
                (dev - dev.round()).abs() < 1e-3,
                "scale {s} pct {pct}: {dev}"
            );
        }
    }
}

#[test]
fn the_track_is_invisible_when_idle() {
    let h = harness(1.0);
    open(&h);
    let (x, y) = (
        (h.comp.get_track_x() + 100.0) as u32,
        (h.comp.get_track_y() + 3.0) as u32,
    );
    let idle = h.render().px(x, y);
    assert!(Frame::near(idle, SIDE_FLAT, 1), "{idle:?}");
    set_running(&h, 0);
    let on = h.render().px(x, y);
    assert!(!Frame::near(on, SIDE_FLAT, 8), "{on:?}");
    set_running(&h, 100);
    let filled = h.render().px(x, y);
    assert!(Frame::near(filled, ACCENT_SOLID, 1), "{filled:?}");
}

#[test]
fn buttons_are_right_aligned_8_apart() {
    for s in SCALES {
        let h = harness(s);
        open(&h);
        let right = h.comp.get_box_x() + h.comp.get_box_w() - 20.0;
        let ir = h.comp.get_install_x() + h.comp.get_install_w();
        assert!((ir - right).abs() < 0.01, "scale {s}: {ir} vs {right}");
        let gap = h.comp.get_install_x() - (h.comp.get_cancel_x() + h.comp.get_cancel_w());
        assert!((gap - 8.0).abs() < 0.01, "scale {s}: {gap}");
    }
}

#[test]
fn cancel_reads_close_with_a_notice_and_is_disabled_when_it_cannot_cancel() {
    let h = harness(1.0);
    open(&h);
    h.comp.set_cancel_label("Close".into());
    h.comp.set_active(true);
    h.comp
        .set_status("The update could not be installed".into());
    h.advance(300);
    settle(&h);
    let (cx, cy) = cancel_centre(&h);
    h.click(cx, cy);
    assert_eq!(h.comp.get_cancels(), 1);

    // Running and abortable: Cancel stops the download.
    set_running(&h, 20);
    h.comp.set_cancel_label("Cancel".into());
    let (cx, cy) = cancel_centre(&h);
    h.click(cx, cy);
    assert_eq!(h.comp.get_aborts(), 1);
    assert_eq!(h.comp.get_cancels(), 1);

    // Running and not abortable: dimmed, silent, and it explains itself on hover.
    let enabled_px = h.render().px(cx as u32, (cy - 12.0) as u32);
    h.comp.set_cancel_enabled(false);
    h.comp.set_cancel_tip("The installer has started".into());
    h.advance(0);
    let dim_px = h.render().px(cx as u32, (cy - 12.0) as u32);
    assert_ne!(enabled_px, dim_px);
    h.click(cx, cy);
    assert_eq!(h.comp.get_aborts(), 1);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_tip_requests(), 1);
    assert_eq!(h.comp.get_last_tip().as_str(), "The installer has started");
    // A disabled Install ignores clicks too.
    let (ix, iy) = install_centre(&h);
    h.click(ix, iy);
    assert_eq!(h.comp.get_installs(), 0);
}

/// The bounding box of the ink (anything unlike the box surface) in a text row of the notes.
fn ink_width(h: &Harness<UpdateWindowPreview>, x_from: f32) -> (u32, u32) {
    let f = h.render();
    let y0 = h.comp.get_notes_y() as u32;
    let y1 = y0 + h.comp.get_notes_h() as u32;
    let x0 = (h.comp.get_box_x() + x_from) as u32;
    let x1 = (h.comp.get_box_x() + h.comp.get_box_w()) as u32 - 4;
    let bb = f
        .bbox_where(x0, y0, x1, y1, |p| !Frame::near(p, SIDE_FLAT, 12))
        .expect("the text is drawn");
    (bb.2 - bb.0 + 1, bb.3 - bb.1 + 1)
}

#[test]
fn entries_are_plain_text() {
    const RAW: &str = "<b>x</b> [a](http://b) <script>1</script>";
    let a = harness(1.0);
    set_sections(&a, vec![section("", &[RAW])]);
    open(&a);
    a.advance(0);
    // The same characters as the empty-note sentence (same size, other colour) measure the same.
    let b = harness(1.0);
    set_sections(&b, vec![]);
    b.comp.set_empty_note(RAW.into());
    open(&b);
    b.advance(0);
    // The entry starts after the dot (4) and the gap (8); the heading row is empty.
    let (w_entry, _) = ink_width(&a, 32.0);
    let (w_note, _) = ink_width(&b, 20.0);
    assert!(w_note > 150, "the markup characters are drawn: {w_note}");
    assert!(
        (i64::from(w_entry) - i64::from(w_note)).abs() <= 1,
        "{w_entry} vs {w_note}"
    );
    // Nothing in it is clickable.
    let f = a.render();
    let x = a.comp.get_box_x() + 40.0;
    let y0 = a.comp.get_notes_y();
    assert!(f.w > 0);
    a.click(x, y0 + 30.0);
    assert_eq!(
        (
            a.comp.get_installs(),
            a.comp.get_cancels(),
            a.comp.get_aborts(),
            a.comp.get_behind_clicks(),
            a.comp.get_tip_requests()
        ),
        (0, 0, 0, 0, 0)
    );
}

#[test]
fn empty_notes_show_the_sentence() {
    let h = harness(1.0);
    set_sections(&h, vec![]);
    open(&h);
    let without = h.comp.get_box_h();
    h.comp
        .set_empty_note("No notes were published with this release.".into());
    settle(&h);
    assert!(h.comp.get_box_h() > without);
    assert!(h.comp.get_notes_h() > 10.0);
    let (w, _) = ink_width(&h, 20.0);
    assert!(w > 150, "{w}");
}

#[test]
fn the_dot_is_4_px_accent_hi() {
    for s in [1.0_f32, 2.0] {
        let h = harness(s);
        set_sections(&h, vec![section("", &["One entry"])]);
        open(&h);
        let f = h.render();
        let x0 = (h.comp.get_box_x() + 18.0) * s;
        let y0 = h.comp.get_notes_y() * s;
        let y1 = y0 + h.comp.get_notes_h() * s;
        let bb = f
            .bbox_where(
                x0 as u32,
                y0 as u32,
                (x0 + 14.0 * s) as u32,
                y1 as u32,
                closer_to_dot_than_surface,
            )
            .expect("a dot is drawn");
        assert_eq!(bb.2 - bb.0 + 1, (4.0 * s) as u32, "scale {s} width");
        assert_eq!(bb.3 - bb.1 + 1, (4.0 * s) as u32, "scale {s} height");
    }
}

/// A pixel that is more the dot's colour than the box surface (the rim of a 4 px circle is blended).
fn closer_to_dot_than_surface(p: [u8; 4]) -> bool {
    let d = |q: [u8; 4]| -> u32 { (0..3).map(|i| u32::from(p[i].abs_diff(q[i]))).sum() };
    d(ACCENT_HI) < d(SIDE_FLAT)
}
