//! The panel slot (Surfaces stage 1), headless: where it sits at every display scaling against the
//! layout maths of `flare-surfaces`, the splitter's drag and clamps and the single refit, the
//! keys, the two viewers (the picture fits and zooms, the text scrolls), the checkerboard, and the
//! one property that says an overlay is open. No OS window, no controller: the properties are set
//! directly, as the window's glue sets them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use flare_surfaces::layout;
use flare_surfaces::overlay::Overlay;
use slint::platform::{Key, PointerEventButton};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::appctl::AppCtl;
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, HelpBridge, SettingsBridge};

const W: u32 = 1000;
const H: u32 = 640;

/// A window with the panel limits handed over, as `AppCtl::new` does.
fn window_at(scale: f32, css_w: u32) -> Harness<AppWindow> {
    let h = Harness::new(scale, css_w, H, || {
        AppWindow::new().expect("the window builds")
    });
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn window(scale: f32) -> Harness<AppWindow> {
    window_at(scale, W)
}

fn solid(w: u32, h: u32, px: [u8; 4]) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    for p in buf.make_mut_slice() {
        *p = Rgba8Pixel {
            r: px[0],
            g: px[1],
            b: px[2],
            a: px[3],
        };
    }
    Image::from_rgba8(buf)
}

fn rows(n: usize) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = (0..n).map(|i| format!("line {i}").into()).collect();
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn open_note(h: &Harness<AppWindow>) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(0);
    h.comp.set_panel_note("Opening".into());
    h.advance(0);
}

fn open_picture(h: &Harness<AppWindow>, w: u32, ph: u32, px: [u8; 4], alpha: bool) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(1);
    h.comp.set_panel_picture(solid(w, ph, px));
    h.comp.set_panel_alpha(alpha);
    h.advance(0);
}

fn open_text(h: &Harness<AppWindow>, n: usize) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(2);
    h.comp.set_panel_lines(rows(n));
    h.comp.set_panel_max_cols(20);
    h.advance(0);
}

fn key(k: Key) -> String {
    String::from(char::from(k))
}

// ---- the layout ----

#[test]
fn nothing_is_drawn_or_reserved_while_the_panel_is_closed() {
    for s in SCALES {
        let h = window(s);
        assert!(!h.comp.get_panel_shown());
        assert_eq!(h.comp.get_panel_slot_w(), 0.0);
        assert_eq!(h.comp.get_panel_visible_w(), 0.0);
        let page = (h.comp.get_page_w() * s).round() as i32;
        assert!(
            (h.comp.get_grid_phys_width() - page).abs() <= 1,
            "scale {s}"
        );
    }
}

#[test]
fn the_slot_matches_the_layout_maths_at_every_scale_width_and_share() {
    for s in SCALES {
        for css_w in [600u32, 840, 1000, 1400, 1920] {
            for share in [None, Some(0.15f32), Some(0.4), Some(0.9)] {
                let h = window_at(s, css_w);
                open_note(&h);
                h.comp.set_panel_share(share.unwrap_or(0.0));
                h.advance(0);
                let page = (h.comp.get_page_w() * s).round() as i32;
                let want = layout::slot_layout(page, f64::from(s), share);
                let got = (h.comp.get_panel_slot_w() * s).round() as i32;
                let at = format!("scale {s}, {css_w} css px wide, share {share:?}");
                assert_eq!(h.comp.get_panel_shown(), want.shown, "{at}");
                assert!(
                    (got - want.slot_w).abs() <= 1,
                    "{at}: slot {got}, maths {}",
                    want.slot_w
                );
                let grid = h.comp.get_grid_phys_width();
                assert!(
                    (grid - want.grid_w).abs() <= 1,
                    "{at}: grid {grid}, maths {}",
                    want.grid_w
                );
                assert!(
                    (grid + got - page).abs() <= 1,
                    "{at}: the slot and the grid fill the page"
                );
            }
        }
    }
}

#[test]
fn the_slot_edges_sit_on_whole_device_pixels() {
    for s in SCALES {
        let h = window(s);
        open_note(&h);
        h.comp.set_panel_share(0.37);
        h.advance(0);
        for v in [
            h.comp.get_panel_slot_w(),
            h.comp.get_panel_slot_x(),
            h.comp.get_panel_splitter_x(),
        ] {
            let dev = v * s;
            assert!(
                (dev - dev.round()).abs() < 0.01,
                "scale {s}: {v} css px is {dev} device px"
            );
        }
    }
}

#[test]
fn the_copied_badge_stays_over_the_terminal_never_under_a_wide_slot() {
    for s in SCALES {
        let h = window(s);
        open_note(&h);
        h.comp.set_panel_share(0.65);
        h.comp.set_copied_phase(1);
        h.advance(0);
        let slot_left = h.comp.get_grid_left() + h.comp.get_panel_slot_x();
        let pill_right = h.comp.get_badge_x() + h.comp.get_badge_w();
        assert!(
            pill_right <= slot_left + 0.5,
            "scale {s}: the pill ends at {pill_right}, the slot begins at {slot_left}"
        );
        assert!(h.comp.get_badge_x() >= h.comp.get_grid_left(), "scale {s}");
    }
}

#[test]
fn a_page_without_room_shows_no_slot_and_keeps_the_whole_grid() {
    for s in SCALES {
        // 400 css px: the terminal keeps 320, which leaves 80, under the 120 a slot needs.
        let h = Harness::new(s, 400, H, || AppWindow::new().unwrap());
        AppCtl::push_panel_limits(&h.comp);
        open_note(&h);
        assert!(!h.comp.get_panel_fits(), "scale {s}");
        assert!(!h.comp.get_panel_shown(), "scale {s}");
        assert_eq!(h.comp.get_panel_slot_w(), 0.0);
    }
}

#[test]
fn a_settings_page_or_the_start_screen_in_front_hides_the_slot() {
    let h = window(1.0);
    open_note(&h);
    assert!(h.comp.get_panel_shown());
    for kind in [1, 2] {
        h.comp.set_front_kind(kind);
        h.advance(0);
        assert!(!h.comp.get_panel_shown(), "front kind {kind}");
        assert_eq!(h.comp.get_panel_slot_w(), 0.0);
    }
    h.comp.set_front_kind(0);
    h.advance(0);
    assert!(h.comp.get_panel_shown());
}

#[test]
fn the_slot_stays_inside_the_page_beside_a_sidebar() {
    let h = window(1.0);
    h.comp.set_has_tabs(true);
    h.comp.set_layout_vertical(true);
    open_note(&h);
    h.advance(0);
    let left = h.comp.get_page_left();
    assert!(left > 100.0, "the sidebar takes room: {left}");
    let slot_right = left + h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w();
    assert!((slot_right - W as f32).abs() <= 1.0);
    let want = layout::slot_layout((h.comp.get_page_w()).round() as i32, 1.0, None);
    assert!((h.comp.get_panel_slot_w().round() as i32 - want.slot_w).abs() <= 1);
}

// ---- the splitter ----

fn splitter_point(h: &Harness<AppWindow>) -> (f32, f32) {
    (
        h.comp.get_panel_splitter_x() + h.comp.get_panel_splitter_w() / 2.0,
        h.comp.get_grid_top() + 200.0,
    )
}

#[test]
fn the_splitter_follows_the_pointer_clamps_and_the_grid_waits_for_the_release() {
    for s in [1.0f32, 1.5, 2.25, 3.0] {
        let h = window(s);
        open_note(&h);
        let resized = Rc::new(Cell::new(0u32));
        {
            let resized = resized.clone();
            h.comp
                .on_grid_resized(move |_, _, _| resized.set(resized.get() + 1));
        }
        let released = Rc::new(Cell::new(None::<f32>));
        {
            let released = released.clone();
            let weak = h.comp.as_weak();
            h.comp.on_panel_splitter_released(move |share| {
                released.set(Some(share));
                if let Some(c) = weak.upgrade() {
                    c.set_panel_share(share);
                }
            });
        }
        h.advance(0);
        let base = resized.get();
        let slot0 = h.comp.get_panel_slot_w();
        let grid0 = h.comp.get_grid_phys_width();
        let (x, y) = splitter_point(&h);
        h.move_to(x, y);
        h.press(x, y, PointerEventButton::Left);
        // Wider: the slot grows by what the pointer moved; the grid keeps its width (rule 24).
        h.move_to(x - 100.0, y);
        h.advance(0);
        let grown = h.comp.get_panel_visible_w();
        assert!(
            (grown - (slot0 + 100.0)).abs() <= 1.0,
            "scale {s}: {grown} after {slot0} + 100"
        );
        assert_eq!(
            h.comp.get_grid_phys_width(),
            grid0,
            "scale {s}: the grid held while dragging"
        );
        assert_eq!(resized.get(), base, "scale {s}: no fit while dragging");
        // Past the widest and the narrowest: clamped to the limits, not to the pointer.
        h.move_to(x - 5000.0, y);
        h.advance(0);
        let max = layout::limits((h.comp.get_page_w() * s).round() as i32, f64::from(s)).1;
        assert!(
            ((h.comp.get_panel_visible_w() * s).round() as i32 - max).abs() <= 1,
            "scale {s}: widest"
        );
        h.move_to(x + 5000.0, y);
        h.advance(0);
        let min = layout::limits((h.comp.get_page_w() * s).round() as i32, f64::from(s)).0;
        assert!(
            ((h.comp.get_panel_visible_w() * s).round() as i32 - min).abs() <= 1,
            "scale {s}: narrowest"
        );
        // Back to a width in between, then let go.
        h.move_to(x - 60.0, y);
        h.advance(0);
        let drag = h.comp.get_panel_visible_w();
        h.release(x - 60.0, y, PointerEventButton::Left);
        h.advance(0);
        let share = released.get().expect("the release reports the share");
        assert!(
            (share - drag / h.comp.get_page_w()).abs() < 0.002,
            "scale {s}"
        );
        assert!(!h.comp.get_panel_splitting());
        assert!(
            (h.comp.get_panel_slot_w() - drag).abs() <= 1.0,
            "scale {s}: the settled slot is the dragged one"
        );
        assert!(h.comp.get_grid_phys_width() != grid0);
        assert_eq!(
            resized.get(),
            base + 1,
            "scale {s}: one fit after the release"
        );
    }
}

#[test]
fn a_right_click_on_the_splitter_does_not_start_a_drag() {
    let h = window(1.0);
    open_note(&h);
    let (x, y) = splitter_point(&h);
    h.move_to(x, y);
    h.press(x, y, PointerEventButton::Right);
    h.move_to(x - 80.0, y);
    h.advance(0);
    assert!(!h.comp.get_panel_splitting());
    h.release(x - 80.0, y, PointerEventButton::Right);
}

// ---- focus and keys ----

#[test]
fn the_panel_takes_the_keys_on_request_and_escape_asks_for_them_back() {
    let h = window(1.0);
    h.activate();
    open_text(&h, 50);
    assert!(
        !h.comp.get_panel_has_focus(),
        "a panel never takes the keys on its own"
    );
    let escapes = Rc::new(Cell::new(0));
    {
        let escapes = escapes.clone();
        h.comp
            .on_panel_escape(move || escapes.set(escapes.get() + 1));
    }
    h.comp.invoke_focus_panel();
    h.advance(0);
    assert!(h.comp.get_panel_has_focus());
    h.key_text(&key(Key::Escape));
    assert_eq!(escapes.get(), 1);
    // A plain letter is nobody's here: it is not taken and nothing escapes.
    h.key_text("a");
    assert_eq!(escapes.get(), 1);
}

#[test]
fn a_click_in_the_panel_asks_for_the_keys() {
    let h = window(1.0);
    open_text(&h, 50);
    let pressed = Rc::new(Cell::new(0));
    {
        let pressed = pressed.clone();
        h.comp
            .on_panel_pressed(move || pressed.set(pressed.get() + 1));
    }
    let x = h.comp.get_panel_body_x() + h.comp.get_panel_body_w() / 2.0;
    let y = h.comp.get_panel_body_y() + 100.0;
    h.click(x, y);
    assert_eq!(pressed.get(), 1);
    assert!(h.comp.get_panel_has_focus() || pressed.get() == 1);
}

#[test]
fn the_close_button_closes_and_the_header_names_the_file() {
    let h = window(1.0);
    open_text(&h, 5);
    h.comp.set_panel_title("notes.txt".into());
    h.comp.set_panel_sub("5 lines, 40 B".into());
    h.advance(0);
    let closed = Rc::new(Cell::new(0));
    {
        let closed = closed.clone();
        h.comp.on_panel_close(move || closed.set(closed.get() + 1));
    }
    let slot_right =
        h.comp.get_page_left() + h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w();
    let x = slot_right - 8.0 - 10.0;
    let y = h.comp.get_grid_top() + h.comp.get_panel_header_h() / 2.0;
    h.click(x, y);
    assert_eq!(closed.get(), 1);
}

// ---- the text viewer ----

#[test]
fn the_arrow_keys_the_page_keys_and_home_and_end_scroll_the_text() {
    for s in [1.0f32, 2.25] {
        let h = window(s);
        h.activate();
        open_text(&h, 1000);
        h.comp.invoke_focus_panel();
        h.advance(0);
        assert_eq!(h.comp.get_panel_first_line(), 0);
        for _ in 0..3 {
            h.key_text(&key(Key::DownArrow));
        }
        h.advance(0);
        assert_eq!(
            h.comp.get_panel_first_line(),
            3,
            "scale {s}: three rows down"
        );
        h.key_text(&key(Key::UpArrow));
        h.advance(0);
        assert_eq!(h.comp.get_panel_first_line(), 2, "scale {s}");
        h.key_text(&key(Key::PageDown));
        h.advance(0);
        let after_page = h.comp.get_panel_first_line();
        assert!(
            after_page > 20,
            "scale {s}: a page is many rows ({after_page})"
        );
        h.key_text(&key(Key::End));
        h.advance(0);
        let last = h.comp.get_panel_first_line();
        assert!(
            last > 900,
            "scale {s}: the end shows the last rows ({last})"
        );
        h.key_text(&key(Key::DownArrow));
        h.advance(0);
        assert_eq!(
            h.comp.get_panel_first_line(),
            last,
            "scale {s}: the end stops"
        );
        h.key_text(&key(Key::Home));
        h.advance(0);
        assert_eq!(h.comp.get_panel_first_line(), 0, "scale {s}");
    }
}

#[test]
fn the_gutter_widens_with_the_number_of_lines() {
    let h = window(1.0);
    open_text(&h, 50);
    let small = h.comp.get_panel_gutter_w();
    open_text(&h, 20_000);
    let big = h.comp.get_panel_gutter_w();
    assert!(big > small, "{big} against {small}");
}

#[test]
fn a_big_file_draws_only_the_rows_in_view() {
    // 100 000 lines: the window, not the file, decides how much is built. The proof is time: this
    // test would not finish if the rows were all made.
    let h = window(1.0);
    open_text(&h, 100_000);
    let f = h.render();
    let x0 = (h.comp.get_panel_body_x() * 1.0) as u32;
    let ink = f.count_where(x0, (h.comp.get_panel_body_y()) as u32, W, H, |p| {
        !Frame::near(p, [23, 23, 23, 255], 6)
    });
    assert!(ink > 100, "the first rows are drawn ({ink} inked pixels)");
}

// ---- the picture viewer ----

fn image_centre(h: &Harness<AppWindow>) -> (f32, f32) {
    (
        h.comp.get_panel_image_x() + h.comp.get_panel_image_w() / 2.0,
        h.comp.get_panel_image_y() + h.comp.get_panel_image_h() / 2.0,
    )
}

#[test]
fn a_big_picture_fits_the_slot_and_a_click_shows_it_at_one_pixel_to_one_pixel() {
    for s in [1.0f32, 1.5, 2.25, 3.0] {
        let h = window(s);
        open_picture(&h, 2400, 1200, [200, 30, 40, 255], false);
        assert!(h.comp.get_panel_image_can_zoom(), "scale {s}");
        assert!(!h.comp.get_panel_image_zoomed());
        let body_w = h.comp.get_panel_body_w();
        let body_h = h.comp.get_panel_body_h();
        let (w, ph) = (h.comp.get_panel_image_w(), h.comp.get_panel_image_h());
        assert!(
            w <= body_w + 0.5 && ph <= body_h + 0.5,
            "scale {s}: fits {w}x{ph} in {body_w}x{body_h}"
        );
        assert!(
            ((w / ph) - 2.0).abs() < 0.05,
            "scale {s}: the shape is kept ({w} x {ph})"
        );
        let (cx, cy) = image_centre(&h);
        h.click(cx, cy);
        h.advance(0);
        assert!(
            h.comp.get_panel_image_zoomed(),
            "scale {s}: a click zooms in"
        );
        assert!(
            ((h.comp.get_panel_image_w() * s).round() - 2400.0).abs() <= 1.0,
            "scale {s}: one image pixel is one device pixel, not {}",
            h.comp.get_panel_image_w() * s
        );
        let (cx, cy) = (
            h.comp.get_panel_body_x() + body_w / 2.0,
            h.comp.get_panel_body_y() + body_h / 2.0,
        );
        h.click(cx, cy);
        h.advance(0);
        assert!(
            !h.comp.get_panel_image_zoomed(),
            "scale {s}: a click zooms out"
        );
    }
}

#[test]
fn a_display_scaling_change_with_the_panel_open_keeps_the_layout_and_the_one_to_one_zoom() {
    for (from, to) in [(1.0f32, 2.25f32), (2.25, 1.0), (1.5, 3.0)] {
        let mut h = window(from);
        open_picture(&h, 2400, 1200, [200, 30, 40, 255], false);
        let (cx, cy) = image_centre(&h);
        h.click(cx, cy);
        h.advance(0);
        assert!(h.comp.get_panel_image_zoomed(), "{from}: zoomed");
        h.set_scale(to);
        h.advance(0);
        let at = format!("{from} -> {to}");
        let page = (h.comp.get_page_w() * to).round() as i32;
        let want = layout::slot_layout(page, f64::from(to), None);
        let got = (h.comp.get_panel_slot_w() * to).round() as i32;
        assert!(
            (got - want.slot_w).abs() <= 1,
            "{at}: slot {got}, maths {}",
            want.slot_w
        );
        let grid = h.comp.get_grid_phys_width();
        assert!(
            (grid - want.grid_w).abs() <= 1,
            "{at}: grid {grid}, maths {}",
            want.grid_w
        );
        assert!(
            ((h.comp.get_panel_image_w() * to).round() - 2400.0).abs() <= 1.0,
            "{at}: one image pixel is one device pixel, not {}",
            h.comp.get_panel_image_w() * to
        );
    }
}

#[test]
fn a_small_picture_is_shown_as_it_is_and_a_click_changes_nothing() {
    for s in [1.0f32, 2.25] {
        let h = window(s);
        open_picture(&h, 64, 48, [10, 200, 10, 255], false);
        assert!(!h.comp.get_panel_image_can_zoom());
        assert!(
            ((h.comp.get_panel_image_w() * s).round() - 64.0).abs() <= 1.0,
            "scale {s}"
        );
        let (cx, cy) = image_centre(&h);
        h.click(cx, cy);
        h.advance(0);
        assert!(!h.comp.get_panel_image_zoomed());
    }
}

#[test]
fn a_picture_with_alpha_sits_on_a_checkerboard_and_an_opaque_one_does_not() {
    let sample = |alpha: bool, px: [u8; 4]| -> Vec<[u8; 4]> {
        let h = window(1.0);
        // The tile the controller draws under a picture with transparency.
        h.comp.set_panel_checker(crate::panel_ctl::checker_tile());
        open_picture(&h, 160, 160, px, alpha);
        let f = h.render();
        let (cx, cy) = image_centre(&h);
        // A row of points across the picture.
        (0..160u32)
            .step_by(4)
            .map(|i| f.px((cx - 80.0) as u32 + i, cy as u32))
            .collect()
    };
    let clear = sample(true, [0, 0, 0, 0]);
    let greys: std::collections::BTreeSet<[u8; 4]> = clear.iter().copied().collect();
    assert!(
        greys.contains(&[0x9a, 0x9a, 0x9a, 255]) && greys.contains(&[0x7a, 0x7a, 0x7a, 255]),
        "both greys of the board show under a clear picture: {greys:?}"
    );
    let solid_red = sample(false, [200, 30, 40, 255]);
    assert!(
        solid_red
            .iter()
            .all(|p| Frame::near(*p, [200, 30, 40, 255], 2)),
        "{solid_red:?}"
    );
    // A half clear picture shows the board through it.
    let half = sample(true, [200, 30, 40, 128]);
    let distinct: std::collections::BTreeSet<[u8; 4]> = half.iter().copied().collect();
    assert!(distinct.len() >= 2, "the board shows through: {distinct:?}");
}

// ---- the overlay property ----

fn set_overlay(c: &AppWindow, which: Overlay, on: bool) {
    // No catch-all arm: a new overlay in flare-surfaces fails to compile here until it is counted.
    match which {
        Overlay::Menu => c.set_menu_open(on),
        Overlay::Overflow => c.set_overflow_open(on),
        Overlay::PluginsDropdown => c.set_plugins_open(on),
        Overlay::Dialog => c.set_dialog_open(on),
        Overlay::PluginAsk => c.set_pask_open(on),
        Overlay::UpdateWindow => c.set_update_open(on),
        Overlay::UpdateNotice => c.set_update_notice(if on { "A new version" } else { "" }.into()),
        Overlay::StagedToast => {
            c.set_staged_toast(if on { "Installed abc1234" } else { "" }.into())
        }
        Overlay::Settings => c.set_front_kind(if on { 1 } else { 0 }),
        Overlay::SettingsSelect => c.global::<SettingsBridge>().set_select_open(on),
        Overlay::ThemeEditor => c.global::<SettingsBridge>().set_editor_open(on),
        Overlay::SettingsAsk => c.global::<SettingsBridge>().set_ask_open(on),
        Overlay::DictationAsk => c.global::<SettingsBridge>().set_dict_ask_open(on),
        Overlay::PluginSheet => c.global::<SettingsBridge>().set_sheet_open(on),
        Overlay::ColourPicker => c.global::<SettingsBridge>().set_picker_open(on),
        Overlay::Help => c.global::<HelpBridge>().set_open(on),
        Overlay::Find => c.set_find_open(on),
        Overlay::Diagnostics => c.set_overlay_visible(on),
        Overlay::TabDrag => c.set_carrying(on),
        Overlay::SidebarOverSlot => {
            // Vertical tabs, collapsed, out on a hover, and wider than the terminal beside the panel.
            c.set_layout_vertical(true);
            c.set_has_tabs(true);
            c.set_sidebar_collapsed(true);
            c.set_sidebar_width(2000.0);
            c.set_sidebar_revealed(on);
        }
    }
}

#[test]
fn overlay_open_is_false_with_nothing_open() {
    let h = window(1.0);
    assert!(!h.comp.get_overlay_open());
}

#[test]
fn every_overlay_turns_overlay_open_on_and_off_again() {
    for which in Overlay::ALL {
        let h = window(1.0);
        open_note(&h);
        set_overlay(&h.comp, which, true);
        h.advance(0);
        assert!(h.comp.get_overlay_open(), "{which:?} open");
        set_overlay(&h.comp, which, false);
        h.advance(0);
        assert!(!h.comp.get_overlay_open(), "{which:?} closed");
    }
}

#[test]
fn two_overlays_keep_it_on_until_both_close() {
    let h = window(1.0);
    set_overlay(&h.comp, Overlay::Menu, true);
    set_overlay(&h.comp, Overlay::Find, true);
    set_overlay(&h.comp, Overlay::Menu, false);
    h.advance(0);
    assert!(h.comp.get_overlay_open());
    set_overlay(&h.comp, Overlay::Find, false);
    h.advance(0);
    assert!(!h.comp.get_overlay_open());
}

#[test]
fn overlay_open_is_read_only_for_rust_and_computed_in_the_window_file() {
    // The property is an `out` one: the glue cannot set it, so it cannot drift from the overlays.
    let app = include_str!("../../ui/app.slint");
    let line = app
        .lines()
        .find(|l| l.contains("property <bool> overlay-open:"))
        .expect("the property is declared");
    assert!(line.trim_start().starts_with("out property"), "{line}");
    for term in [
        "root.menu-open",
        "root.overflow-open",
        "root.plugins-open",
        "root.dialog-open",
        "root.pask-open",
        "root.update-open",
        "root.update-notice",
        "root.front-kind == 1",
        "SettingsBridge.select-open",
        "SettingsBridge.editor-open",
        "SettingsBridge.ask-open",
        "SettingsBridge.dict-ask-open",
        "SettingsBridge.sheet-open",
        "SettingsBridge.picker-open",
        "HelpBridge.open",
        "root.find-open",
        "root.overlay-visible",
        "root.carrying",
        "root.sidebar-over-slot",
    ] {
        assert!(line.contains(term), "overlay-open misses {term}");
    }
    assert_eq!(
        line.matches("||").count() + 1,
        Overlay::ALL.len(),
        "one term per overlay in flare-surfaces"
    );
}

// ---- what the file says ----

#[test]
fn the_slot_is_declared_in_the_page_below_the_help_panel_and_the_find_bar() {
    let app = include_str!("../../ui/app.slint");
    let slot = app.find("panel-slot := PanelSlot").expect("the slot");
    let help = app.find("help-panel := HelpPanel").expect("the help panel");
    let find = app.find("find := FindBar").expect("the find bar");
    assert!(
        slot < help && slot < find,
        "the slot is under the help panel and the find bar"
    );
    // The find bar spans the terminal only.
    assert!(
        app[find..]
            .lines()
            .take(6)
            .any(|l| l.contains("root.grid-w"))
    );
}

#[test]
fn the_panel_files_use_no_animation_and_no_timer() {
    let src = include_str!("../../ui/panel.slint");
    assert!(
        !src.contains("animate"),
        "a closed or idle panel runs nothing"
    );
    assert!(!src.contains("Timer"));
}

// ---- the web panel (Surfaces stage 3) ----

fn open_web(h: &Harness<AppWindow>) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(3);
    h.comp.set_panel_web_url("https://a.example/".into());
    h.advance(0);
}

/// The browser window's rectangle as the window lays it out, in device pixels.
fn web_box(h: &Harness<AppWindow>) -> flare_surfaces::web::PhysRect {
    flare_surfaces::web::phys_rect(
        h.comp.get_panel_web_x(),
        h.comp.get_panel_web_y(),
        h.comp.get_panel_web_w(),
        h.comp.get_panel_web_h(),
        h.comp.get_scale(),
    )
}

#[test]
fn the_browser_bar_is_part_of_a_web_panels_head_and_of_nothing_else() {
    for s in SCALES {
        let h = window(s);
        open_note(&h);
        assert_eq!(
            h.comp.get_panel_bar_h(),
            0.0,
            "scale {s}: a note has no bar"
        );
        open_picture(&h, 40, 30, [1, 2, 3, 255], false);
        assert_eq!(h.comp.get_panel_bar_h(), 0.0, "scale {s}: nor a picture");
        open_web(&h);
        let bar = h.comp.get_panel_bar_h();
        assert!(bar > 0.0, "scale {s}");
        assert!(
            ((bar * s) - (bar * s).round()).abs() < 0.01,
            "scale {s}: on whole device pixels"
        );
        // The body, and so the page, starts under the header and the bar.
        let top = h.comp.get_panel_slot_top();
        let want = top + h.comp.get_panel_header_h() + bar;
        assert!((h.comp.get_panel_body_y() - want).abs() < 0.01, "scale {s}");
    }
}

/// Max 2026-10-27: a web page has no header (no title row, no agent ribbon); the address row is the top
/// of the panel and holds the close at its right end. A mini-app keeps its header (it has no bar).
#[test]
fn a_web_page_has_no_header_and_the_address_row_holds_the_close() {
    for s in SCALES {
        let h = window(s);
        open_web(&h);
        h.comp.set_panel_title("Glide test".into());
        h.advance(0);
        assert_eq!(h.comp.get_panel_header_h(), 0.0, "scale {s}");
        let top = h.comp.get_panel_slot_top();
        let bar = h.comp.get_panel_bar_h();
        assert!(
            (h.comp.get_panel_body_y() - (top + bar)).abs() < 0.01,
            "scale {s}: the page starts right under the address row"
        );
        // The close is the address row's last button, at its right end.
        let slot_right =
            h.comp.get_page_left() + h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w();
        let cx = h.comp.get_panel_web_close_x();
        let cw = h.comp.get_panel_web_close_w();
        assert!((cw - 24.0).abs() < 1.0, "scale {s}: an icon button ({cw})");
        assert!(
            (slot_right - (cx + cw) - 8.0).abs() < 1.0,
            "scale {s}: 8 px from the right edge ({cx} + {cw}, {slot_right})"
        );
        let closed = Rc::new(Cell::new(0));
        {
            let closed = closed.clone();
            h.comp.on_panel_close(move || closed.set(closed.get() + 1));
        }
        h.click(cx + cw / 2.0, top + bar / 2.0);
        assert_eq!(closed.get(), 1, "scale {s}: the close closes");
        // A mini-app has no bar, so it keeps the header and its close.
        h.comp.set_panel_web_app(true);
        h.advance(0);
        assert!(h.comp.get_panel_header_h() > 0.0, "scale {s}");
        assert_eq!(h.comp.get_panel_bar_h(), 0.0, "scale {s}");
    }
}

#[test]
fn the_page_fills_the_body_less_the_splitters_reach_at_every_scale() {
    for s in SCALES {
        let h = window(s);
        open_web(&h);
        let body = flare_surfaces::web::phys_rect(
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
            h.comp.get_panel_body_h(),
            s,
        );
        let page = web_box(&h);
        let inset = flare_surfaces::web::web_inset_px(f64::from(s));
        assert_eq!(
            page.x,
            body.x + inset,
            "scale {s}: after the splitter's reach"
        );
        // The splitter's whole grab zone lies left of the page: a press in it can only be the splitter's.
        let splitter_right =
            ((h.comp.get_panel_splitter_x() + h.comp.get_panel_splitter_w()) * s).round() as i32;
        assert!(
            page.x >= splitter_right,
            "scale {s}: the page starts at {} under the grab zone that ends at {splitter_right}",
            page.x
        );
        assert_eq!(page.right(), body.right(), "scale {s}");
        assert_eq!(
            (page.y, page.bottom()),
            (body.y, body.bottom()),
            "scale {s}"
        );
        // And it ends on the window's own right and bottom edge.
        assert_eq!(page.right(), (W as f32 * s).round() as i32, "scale {s}");
        assert_eq!(page.bottom(), (H as f32 * s).round() as i32, "scale {s}");
    }
}

#[test]
fn the_page_rectangle_equals_the_layout_maths_at_every_scale_width_and_share() {
    for s in SCALES {
        for css_w in [700u32, 1000, 1400, 1920] {
            for share in [None, Some(0.2f32), Some(0.55)] {
                let h = window_at(s, css_w);
                open_web(&h);
                h.comp.set_panel_share(share.unwrap_or(0.0));
                h.advance(0);
                let k = |v: f32| (v * s).round() as i32;
                let want = flare_surfaces::web::web_rect(
                    k(h.comp.get_page_left()),
                    k(h.comp.get_panel_slot_top()),
                    k(h.comp.get_page_w()),
                    k(h.comp.get_panel_slot_h()),
                    f64::from(s),
                    share,
                    k(h.comp.get_panel_header_h() + h.comp.get_panel_bar_h()),
                )
                .expect("room for a slot");
                let got = web_box(&h);
                let at = format!("scale {s}, {css_w} css px, share {share:?}: {got:?} vs {want:?}");
                assert!(
                    (got.x - want.x).abs() <= 1
                        && (got.y - want.y).abs() <= 1
                        && (got.w - want.w).abs() <= 1
                        && (got.h - want.h).abs() <= 1,
                    "{at}"
                );
            }
        }
    }
}

#[test]
fn the_page_follows_the_splitter_while_it_moves() {
    let h = window(1.0);
    open_web(&h);
    let before = web_box(&h);
    let (x, y) = splitter_point(&h);
    h.move_to(x, y);
    h.press(x, y, PointerEventButton::Left);
    h.move_to(x - 120.0, y);
    h.advance(0);
    // While the pointer is down the page is already where the slot is drawn.
    let after = web_box(&h);
    assert!(after.w > before.w + 100, "{before:?} {after:?}");
    assert_eq!(after.right(), before.right(), "the right edge stays");
    h.release(x - 120.0, y, PointerEventButton::Left);
}

#[test]
fn the_still_is_drawn_in_the_page_box_and_only_while_it_is_set() {
    let h = window(1.0);
    open_web(&h);
    let page = web_box(&h);
    assert!(!h.comp.get_panel_web_has_still());
    h.comp
        .set_panel_web_still(solid(page.w as u32, page.h as u32, [250, 10, 10, 255]));
    h.comp.set_panel_web_has_still(true);
    let frame = h.render();
    let (cx, cy) = (page.x + page.w / 2, page.y + page.h / 2);
    assert!(
        Frame::near(frame.px(cx as u32, cy as u32), [250, 10, 10, 255], 4),
        "the still fills the page box"
    );
    // Outside the box (the header) it is not.
    let header_px = frame.px((page.x + page.w / 2) as u32, (page.y - 50).max(2) as u32);
    assert!(!Frame::near(header_px, [250, 10, 10, 255], 4));
    h.comp.set_panel_web_has_still(false);
    h.comp.set_panel_web_still(Image::default());
    let frame = h.render();
    assert!(!Frame::near(
        frame.px(cx as u32, cy as u32),
        [250, 10, 10, 255],
        4
    ));
}

#[test]
fn the_address_field_shows_the_page_and_enter_and_escape_do_their_part() {
    let h = window(1.0);
    h.activate();
    let submitted = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let escaped = Rc::new(Cell::new(0u32));
    {
        let s = submitted.clone();
        h.comp
            .on_panel_web_submit(move |t| s.borrow_mut().push(t.to_string()));
        let e = escaped.clone();
        h.comp.on_panel_escape(move || e.set(e.get() + 1));
    }
    open_web(&h);
    assert_eq!(h.comp.get_panel_web_draft().as_str(), "https://a.example/");
    // The page moves on while the field is not being typed in: the field follows.
    h.comp.set_panel_web_url("https://b.example/".into());
    h.advance(0);
    assert_eq!(h.comp.get_panel_web_draft().as_str(), "https://b.example/");
    // Typing replaces the selected address; Enter sends what is there.
    h.comp.invoke_focus_address();
    h.advance(0);
    assert!(h.comp.get_panel_web_input_focused());
    assert!(
        h.comp.get_find_input_focused(),
        "the keys are Slint's while the field has them"
    );
    h.key_text("c.example");
    h.key_text(&key(Key::Return));
    assert_eq!(submitted.borrow().as_slice(), ["c.example"]);
    // The page moving on does not overwrite what is being typed.
    h.key_text("x");
    h.comp.set_panel_web_url("https://d.example/".into());
    h.advance(0);
    assert!(h.comp.get_panel_web_draft().contains('x'));
    // Escape puts the address of the page back and hands the keys on.
    h.key_text(&key(Key::Escape));
    assert_eq!(escaped.get(), 1);
    assert_eq!(h.comp.get_panel_web_draft().as_str(), "https://d.example/");
}

#[test]
fn the_bar_buttons_call_back_and_a_dead_one_does_not() {
    let h = window(1.0);
    open_web(&h);
    let calls = Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
    {
        let c = calls.clone();
        h.comp
            .on_panel_web_back(move || c.borrow_mut().push("back"));
        let c = calls.clone();
        h.comp
            .on_panel_web_forward(move || c.borrow_mut().push("forward"));
        let c = calls.clone();
        h.comp
            .on_panel_web_reload(move || c.borrow_mut().push("reload"));
        let c = calls.clone();
        h.comp
            .on_panel_web_stop(move || c.borrow_mut().push("stop"));
    }
    let top = h.comp.get_panel_slot_top() + h.comp.get_panel_header_h();
    let left = h.comp.get_page_left() + h.comp.get_panel_slot_x();
    let button = |i: f32| (left + 8.0 + 12.0 + i * 26.0, top + 16.0);
    // Back and forward are dimmed and do nothing until the page has a history.
    h.click(button(0.0).0, button(0.0).1);
    h.click(button(1.0).0, button(1.0).1);
    assert!(calls.borrow().is_empty());
    h.comp.set_panel_web_can_back(true);
    h.comp.set_panel_web_can_forward(true);
    h.advance(0);
    h.click(button(0.0).0, button(0.0).1);
    h.click(button(1.0).0, button(1.0).1);
    h.click(button(2.0).0, button(2.0).1);
    // While a page loads the third button stops it.
    h.comp.set_panel_web_loading(true);
    h.advance(0);
    h.click(button(2.0).0, button(2.0).1);
    assert_eq!(
        calls.borrow().as_slice(),
        ["back", "forward", "reload", "stop"]
    );
}

#[test]
fn the_download_button_shows_only_for_a_missing_runtime() {
    let h = window(1.0);
    open_web(&h);
    h.comp.set_panel_note("No runtime".into());
    let hits = Rc::new(Cell::new(0u32));
    {
        let c = hits.clone();
        h.comp.on_panel_web_download(move || c.set(c.get() + 1));
    }
    let body_x = h.comp.get_panel_body_x();
    let body_y = h.comp.get_panel_body_y();
    let (bw, bh) = (h.comp.get_panel_body_w(), h.comp.get_panel_body_h());
    let at = (body_x + bw / 2.0, body_y + bh / 2.0 + 70.0);
    h.click(at.0, at.1);
    assert_eq!(hits.get(), 0, "no button without the flag");
    h.comp.set_panel_web_missing_runtime(true);
    h.advance(0);
    h.click(at.0, at.1);
    assert_eq!(hits.get(), 1);
}

#[test]
fn the_popup_strip_takes_the_pages_top_and_its_close_calls_back() {
    let h = window(1.0);
    open_web(&h);
    let page_y = h.comp.get_panel_web_y();
    let page_h = h.comp.get_panel_web_h();
    assert_eq!(
        h.comp.get_panel_popup_h(),
        0.0,
        "no strip while no pop-up is open"
    );
    let closed = Rc::new(Cell::new(0u32));
    {
        let c = closed.clone();
        h.comp.on_panel_web_popup_close(move || c.set(c.get() + 1));
    }
    h.comp
        .set_panel_web_popup("Pop-up from bank.example".into());
    h.advance(0);
    let strip = h.comp.get_panel_popup_h();
    assert!(strip >= 24.0, "{strip}");
    assert_eq!(
        h.comp.get_panel_web_y(),
        page_y + strip,
        "the pop-up sits under the strip"
    );
    assert_eq!(h.comp.get_panel_web_h(), page_h - strip);
    // Close sits at the strip's right end, in Flare's chrome above the page.
    let right = h.comp.get_page_left() + h.comp.get_panel_slot_x() + h.comp.get_panel_slot_w();
    h.click(right - 8.0 - 12.0, page_y + strip / 2.0);
    assert_eq!(closed.get(), 1);
    h.comp.set_panel_web_popup("".into());
    h.advance(0);
    assert_eq!(h.comp.get_panel_web_y(), page_y);
    assert_eq!(h.comp.get_panel_popup_h(), 0.0);
}
// ---- "Ask agent" (#33) ----

#[test]
fn ask_agent_is_offered_over_what_can_be_sent_and_only_with_an_agent() {
    for s in SCALES {
        let h = window(s);
        h.comp.set_panel_ask_on(false);
        open_text(&h, 3);
        assert!(!h.comp.get_panel_ask_visible(), "no agent, scale {s}");
        h.comp.set_panel_ask_on(true);
        h.advance(0);
        assert!(h.comp.get_panel_ask_visible(), "a text file, scale {s}");
        h.comp.set_panel_kind(4);
        h.advance(0);
        assert!(
            !h.comp.get_panel_ask_visible(),
            "the Changes panel, scale {s}"
        );
        h.comp.set_panel_kind(0);
        h.advance(0);
        assert!(!h.comp.get_panel_ask_visible(), "a note, scale {s}");
        open_picture(&h, 10, 10, [0, 0, 0, 255], false);
        assert!(h.comp.get_panel_ask_visible(), "a picture, scale {s}");
        open_web(&h);
        assert!(h.comp.get_panel_ask_visible(), "a page, scale {s}");
    }
}

#[test]
fn ask_agent_calls_back_from_the_header_and_from_the_browser_bar() {
    for s in SCALES {
        let h = window(s);
        let pressed = Rc::new(Cell::new(0u32));
        let p = pressed.clone();
        h.comp.on_panel_ask_agent(move || p.set(p.get() + 1));
        h.comp.set_panel_ask_on(true);
        h.advance(0);
        for (n, kind) in [1u32, 2].into_iter().zip(["text", "web"]) {
            if kind == "text" {
                open_text(&h, 3);
            } else {
                open_web(&h);
            }
            let (x, y, w) = (
                h.comp.get_panel_ask_x(),
                h.comp.get_panel_ask_y(),
                h.comp.get_panel_ask_w(),
            );
            assert!(w > 0.0, "{kind}, scale {s}");
            h.click(x + w / 2.0, y + 12.0);
            assert_eq!(pressed.get(), n, "{kind}, scale {s}");
        }
    }
}

#[test]
fn ask_agent_says_on_hover_what_a_click_sends_with_one_fixed_wording_per_kind() {
    for s in SCALES {
        let h = window(s);
        let tips = Rc::new(RefCell::new(Vec::<String>::new()));
        let t = tips.clone();
        h.comp
            .on_panel_ask_tip(move |text, _, _, _, _| t.borrow_mut().push(text.to_string()));
        h.comp.set_panel_ask_on(true);
        h.advance(0);
        for (kind, want) in [
            ("web", "Ask agent about the selection or this page"),
            ("app", "Ask agent about the selection"),
            ("text", "Ask agent about this file"),
        ] {
            match kind {
                "text" => open_text(&h, 3),
                _ => {
                    open_web(&h);
                    h.comp.set_panel_web_app(kind == "app");
                    h.advance(0);
                }
            }
            tips.borrow_mut().clear();
            let (x, y, w) = (
                h.comp.get_panel_ask_x(),
                h.comp.get_panel_ask_y(),
                h.comp.get_panel_ask_w(),
            );
            h.move_to(x + w / 2.0, y + 12.0);
            assert_eq!(tips.borrow().as_slice(), [want], "{kind}, scale {s}");
            h.move_to(1.0, 1.0);
            assert_eq!(
                tips.borrow().last().map(String::as_str),
                Some(""),
                "leaving takes it down, {kind}, scale {s}"
            );
        }
    }
}

#[test]
fn ask_agent_sits_clear_of_the_buttons_beside_it_and_narrows_with_the_panel() {
    for s in SCALES {
        // 1600 css px at 40%: a wide panel. 600 css px at the minimum share: the narrowest it gets.
        for (css_w, share, compact) in [(1600u32, 0.4f32, false), (600, 0.15, true)] {
            let h = window_at(s, css_w);
            h.comp.set_panel_ask_on(true);
            h.comp.set_panel_share(share);
            open_web(&h);
            let (ask_x, ask_w) = (h.comp.get_panel_ask_x(), h.comp.get_panel_ask_w());
            assert!(
                ask_x + ask_w <= h.comp.get_panel_web_close_x(),
                "the bar, {css_w} at scale {s}"
            );
            assert!(
                h.comp.get_panel_web_field_x() + h.comp.get_panel_web_field_w() <= ask_x,
                "the address field, {css_w} at scale {s}"
            );
            assert!(
                h.comp.get_panel_web_field_w() > 40.0,
                "the address stays usable, {css_w} at scale {s}"
            );
            let glyph_alone = ask_w <= 26.0;
            assert_eq!(glyph_alone, compact, "{css_w} at scale {s}: {ask_w}");
            open_text(&h, 3);
            let (ask_x, ask_w) = (h.comp.get_panel_ask_x(), h.comp.get_panel_ask_w());
            assert!(
                ask_x + ask_w <= h.comp.get_panel_wrap_button_x(),
                "left of wrap, {css_w} at scale {s}"
            );
        }
    }
}

#[test]
fn the_ask_note_sits_over_the_terminal_never_the_panel() {
    for s in SCALES {
        let h = window_at(s, 1200);
        open_text(&h, 3);
        h.comp
            .set_ask_note("Answer the agent's question first".into());
        h.advance(0);
        let (x, w) = (h.comp.get_ask_note_x(), h.comp.get_ask_note_w());
        assert!(w > 0.0, "scale {s}");
        assert!(
            x + w <= h.comp.get_panel_slot_x(),
            "clear of the panel, scale {s}: {} + {} over {}",
            x,
            w,
            h.comp.get_panel_slot_x()
        );
    }
}
