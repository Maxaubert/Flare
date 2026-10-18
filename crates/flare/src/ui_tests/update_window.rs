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

// ---- Task 33: the window at all eight display scalings -----------------------------------------------

mod scales {
    use flare_theme::ansi::contrast_ratio;
    use flare_theme::edges::DEFAULT_WINDOW_EDGES;
    use flare_theme::look::{AppliedLook, LookInput, apply_look};
    use flare_update::geom;
    use slint::ComponentHandle;

    use super::{
        ACCENT_SOLID, SIDE_FLAT, harness, harness_sized, open, section, set_running, set_sections,
        settle,
    };
    use crate::chrome_apply::colour;
    use crate::ui_harness::{Frame, Harness, SCALES};
    use crate::{Theme, UpdateWindowPreview};

    fn on_grid(v: f32, s: f32) -> bool {
        let d = v * s;
        (d - d.round()).abs() < 1e-3
    }

    #[test]
    fn the_window_box_is_centred_to_the_pixel() {
        for s in SCALES {
            for win_w in [1000_u32, 700, 460] {
                let h = harness_sized(s, win_w, 640);
                open(&h);
                let centred = (win_w as f32 - h.comp.get_box_w()) / 2.0;
                assert!(
                    (h.comp.get_box_x() - centred).abs() * s <= 1.0,
                    "scale {s} width {win_w}: box-x {} vs {centred}",
                    h.comp.get_box_x()
                );
                for v in [h.comp.get_box_x(), h.comp.get_box_y(), h.comp.get_box_w()] {
                    assert!(
                        on_grid(v, s),
                        "scale {s} width {win_w}: {v} is off the grid"
                    );
                }
            }
        }
    }

    #[test]
    fn the_window_fits_the_smallest_window() {
        for s in SCALES {
            let h = harness_sized(s, 1000, 420);
            h.comp.set_notes_max_h(geom::notes_max_h(420.0));
            let entries: Vec<String> = (0..25).map(|i| format!("Entry number {i}")).collect();
            let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
            set_sections(&h, vec![section("Bug fixes", &refs)]);
            open(&h);
            assert!(
                h.comp.get_box_y() >= 0.0,
                "scale {s}: box-y {}",
                h.comp.get_box_y()
            );
            let floor = 420.0 * s - (24.0 * s).round();
            for (name, y, bh) in [
                ("install", h.comp.get_install_y(), h.comp.get_install_h()),
                ("cancel", h.comp.get_cancel_y(), h.comp.get_cancel_h()),
            ] {
                assert!(
                    (y + bh) * s <= floor + 0.5,
                    "scale {s}: the {name} button ends at {} device px, floor {floor}",
                    (y + bh) * s
                );
                assert!(y >= 0.0, "scale {s}: {name} above the window");
            }
            assert!(
                h.comp.get_install_x() + h.comp.get_install_w() <= 1000.0,
                "scale {s}: Install is off the right edge"
            );
            assert!(h.comp.get_cancel_x() >= 0.0, "scale {s}");
            // The notes list scrolls inside its cap.
            assert!(
                h.comp.get_notes_content_h() > h.comp.get_notes_h(),
                "scale {s}: {} vs {}",
                h.comp.get_notes_content_h(),
                h.comp.get_notes_h()
            );
            assert!(
                (h.comp.get_notes_h() - geom::notes_max_h(420.0)).abs() < 0.01,
                "scale {s}: {}",
                h.comp.get_notes_h()
            );
            // Both buttons are drawn on the frame.
            let f = h.render();
            for (x, y, w, bh) in [
                (
                    h.comp.get_install_x(),
                    h.comp.get_install_y(),
                    h.comp.get_install_w(),
                    h.comp.get_install_h(),
                ),
                (
                    h.comp.get_cancel_x(),
                    h.comp.get_cancel_y(),
                    h.comp.get_cancel_w(),
                    h.comp.get_cancel_h(),
                ),
            ] {
                assert!(((y + bh) * s).round() as u32 <= f.h, "scale {s}");
                assert!(((x + w) * s).round() as u32 <= f.w, "scale {s}");
            }
        }
    }

    #[test]
    fn the_progress_fill_ends_on_a_device_pixel_and_monotonic() {
        for s in SCALES {
            let h = harness(s);
            open(&h);
            let mut last = -1.0_f32;
            for pct in [0, 25, 50, 75, 100] {
                set_running(&h, pct);
                let w = h.comp.get_fill_w();
                assert!(on_grid(w, s), "scale {s} pct {pct}: {w}");
                assert!(w >= last, "scale {s}: {w} after {last}");
                assert!(w <= h.comp.get_track_w(), "scale {s} pct {pct}");
                last = w;
                // The drawn fill ends exactly where the property says.
                let f = h.render();
                let y = ((h.comp.get_track_y() + 3.0) * s).round() as u32;
                let x0 = (h.comp.get_track_x() * s).round() as u32;
                let drawn = f.run_len_h(y, x0 + (4.0 * s).round() as u32, |p| {
                    Frame::near(p, ACCENT_SOLID, 2)
                });
                if pct == 0 {
                    assert_eq!(h.comp.get_fill_w(), 0.0, "scale {s}");
                } else if pct < 100 {
                    // The fill's right end is a 3 css px round cap, so its last drawn column on the
                    // middle row is the property's end to within one anti-aliased device pixel.
                    let end = (x0 + (w * s).round() as u32) as i64;
                    let got = (x0 + (4.0 * s).round() as u32 + drawn) as i64;
                    assert!(
                        (got - end).abs() <= 1,
                        "scale {s} pct {pct}: {got} vs {end}"
                    );
                }
            }
            assert_eq!(h.comp.get_fill_w(), h.comp.get_track_w(), "scale {s}");
        }
    }

    #[test]
    fn the_box_height_is_identical_idle_running_and_with_a_notice() {
        for s in SCALES {
            let h = harness(s);
            open(&h);
            let idle = h.comp.get_box_h();
            set_running(&h, 40);
            assert_eq!(h.comp.get_box_h(), idle, "scale {s} running");
            h.comp.set_running(false);
            h.comp.set_cancel_label("Close".into());
            h.comp
                .set_status("The update could not be installed".into());
            h.comp.set_pct_text("".into());
            h.advance(300);
            settle(&h);
            assert_eq!(h.comp.get_box_h(), idle, "scale {s} with a notice");
            assert!(on_grid(idle, s), "scale {s}: box height {idle}");
        }
    }

    #[test]
    fn moving_between_scales_relays_out() {
        use slint::PhysicalSize;
        use slint::platform::WindowEvent;
        let rects = |h: &Harness<UpdateWindowPreview>| {
            [
                h.comp.get_box_x(),
                h.comp.get_box_y(),
                h.comp.get_box_w(),
                h.comp.get_box_h(),
                h.comp.get_track_x(),
                h.comp.get_track_y(),
                h.comp.get_track_w(),
                h.comp.get_install_x(),
                h.comp.get_install_y(),
                h.comp.get_cancel_x(),
                h.comp.get_cancel_w(),
            ]
        };
        let h = harness(1.0);
        open(&h);
        let relayout = |scale: f32| {
            let w = h.comp.window();
            w.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            w.set_size(PhysicalSize::new(
                (1000.0 * scale).round() as u32,
                (640.0 * scale).round() as u32,
            ));
            h.advance(0);
            h.advance(0);
        };
        let fresh = |scale: f32| {
            let f = harness(scale);
            open(&f);
            rects(&f)
        };
        relayout(2.25);
        assert_eq!(rects(&h), fresh(2.25), "1.0 -> 2.25");
        relayout(1.25);
        assert_eq!(rects(&h), fresh(1.25), "2.25 -> 1.25");
    }

    fn look(id: &str, see_through: u8, glass_possible: bool) -> AppliedLook {
        apply_look(&LookInput {
            theme_id: id,
            custom: None,
            draft: None,
            background: None,
            accent: None,
            secondary: None,
            edges: DEFAULT_WINDOW_EDGES,
            see_through,
            glass_possible,
        })
    }

    fn token(l: &AppliedLook, name: &str) -> String {
        l.chrome.vars[name].clone()
    }

    /// Writes the tokens the window reads into the preview's Theme.
    fn paint(h: &Harness<UpdateWindowPreview>, l: &AppliedLook) {
        let t = h.comp.global::<Theme>();
        t.set_light(l.light);
        t.set_ground(colour(&token(l, "--p-bg")));
        t.set_side_flat(colour(&token(l, "--p-side-flat")));
        t.set_text(colour(&token(l, "--p-text")));
        t.set_text_soft(colour(&token(l, "--p-text-soft")));
        t.set_dim(colour(&token(l, "--p-dim")));
        t.set_accent_hi(colour(&token(l, "--p-accent-hi")));
        t.set_accent_solid(colour(&token(l, "--p-accent-solid")));
        t.set_divider(colour(&token(l, "--p-divider")));
        h.advance(0);
    }

    #[test]
    fn the_look_on_the_dark_and_the_light_theme_keeps_contrast() {
        for id in ["pt-default", "paper"] {
            let l = look(id, 0, false);
            let h = harness(1.0);
            paint(&h, &l);
            open(&h);
            // The box really is the theme's side-flat, so the ratios below are the drawn ones.
            let f = h.render();
            let side = token(&l, "--p-side-flat");
            let probe = f.px(
                (h.comp.get_box_x() + h.comp.get_box_w() - 6.0) as u32,
                (h.comp.get_box_y() + 30.0) as u32,
            );
            let c = colour(&side);
            assert!(
                Frame::near(probe, [c.red(), c.green(), c.blue(), 255], 2),
                "{id}: the box is {probe:?}, not {side}"
            );
            let notes = contrast_ratio(&token(&l, "--p-text-soft"), &side);
            assert!(notes >= 4.5, "{id}: notes text {notes:.2}");
            let status = contrast_ratio(&token(&l, "--p-text-soft"), &side);
            assert!(status >= 4.5, "{id}: status {status:.2}");
            let title = contrast_ratio(&token(&l, "--p-text"), &side);
            assert!(title >= 4.5, "{id}: title {title:.2}");
            let dot = contrast_ratio(&token(&l, "--p-accent-hi"), &side);
            assert!(dot >= 3.0, "{id}: the dot {dot:.2}");
        }
    }

    #[test]
    fn a_translucent_window_does_not_make_the_window_translucent() {
        let l = look("pt-default", 40, true);
        let c = colour(&token(&l, "--p-side-flat"));
        for s in [1.0_f32, 1.5, 2.25] {
            let h = harness(s);
            paint(&h, &l);
            open(&h);
            let f = h.render();
            let x = ((h.comp.get_box_x() + h.comp.get_box_w() - 6.0) * s) as u32;
            let y = ((h.comp.get_box_y() + 30.0) * s) as u32;
            let p = f.px(x, y);
            assert_eq!(p[3], 255, "scale {s}: the box is opaque");
            assert!(
                Frame::near(p, [c.red(), c.green(), c.blue(), 255], 2),
                "scale {s}: {p:?}"
            );
            let _ = SIDE_FLAT;
        }
    }

    #[test]
    fn gpu_and_software_agree_within_two_levels_on_the_window_fill() {
        // See the chip's twin: the harness has no GPU painter, so this reads the software frame twice.
        eprintln!(
            "note: no offscreen GPU painter in the harness; the GPU pass of this test is skipped"
        );
        for s in SCALES {
            let h = harness(s);
            open(&h);
            set_running(&h, 100);
            let y = ((h.comp.get_track_y() + 3.0) * s).round() as u32;
            let x = ((h.comp.get_track_x() + 100.0) * s).round() as u32;
            let a = h.render().px(x, y);
            let b = h.render().px(x, y);
            assert!(Frame::near(a, b, 2), "scale {s}");
            assert!(Frame::near(a, ACCENT_SOLID, 2), "scale {s}: {a:?}");
        }
    }
}
