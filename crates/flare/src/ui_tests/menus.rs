//! component and flow tests for the menus (Tasks 23 and 31).

use slint::{Model, ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{MenuRow, MenusPreview, OverflowRow};

const SIDE_FLAT: [u8; 4] = [0x1f, 0x1f, 0x1e, 0xff];

fn row(label: &str) -> MenuRow {
    MenuRow {
        label: label.into(),
        hint: "".into(),
        icon: "".into(),
        filled_icon: false,
        disabled: false,
        trailing_icon: "".into(),
        trailing_filled: false,
        trailing_tip: "".into(),
        keep_open: false,
        group: false,
        flyout: false,
        flyout_pick: false,
        parent: -1,
        ..Default::default()
    }
}

fn menu_rows(rows: Vec<MenuRow>) -> ModelRc<MenuRow> {
    ModelRc::new(VecModel::from(rows))
}

fn orow(id: &str, needs: bool) -> OverflowRow {
    OverflowRow {
        id: id.into(),
        label: format!("tab {id}").into(),
        folder: "C:\\work".into(),
        needs,
        loading: false,
        settings: false,
        mark_state: 0,
        mark_color: slint::Color::from_argb_u8(0, 0, 0, 0),
        agent: 0,
    }
}

fn harness(scale: f32) -> Harness<MenusPreview> {
    Harness::new(scale, 400, 300, || MenusPreview::new().unwrap())
}

fn open_menu(h: &Harness<MenusPreview>, rows: Vec<MenuRow>, x: f32, y: f32) {
    h.comp.set_menu_rows(menu_rows(rows));
    h.comp.set_menu_x(x);
    h.comp.set_menu_y(y);
    h.comp.set_menu_open(true);
    h.advance(0);
}

fn open_list(h: &Harness<MenusPreview>, rows: Vec<OverflowRow>) {
    h.comp.set_ov_rows(ModelRc::new(VecModel::from(rows)));
    h.comp.set_ov_right(900.0 / 2.0 + 0.0);
    h.comp.set_ov_bottom(36.0);
    h.comp.set_ov_open(true);
    h.advance(0);
}

/// A dropdown from a button whose right edge is `right`, top `top` and foot `foot` (the New tab arrow).
fn open_drop(h: &Harness<MenusPreview>, rows: Vec<MenuRow>, right: f32, top: f32, foot: f32) {
    h.comp.set_menu_drop(true);
    h.comp.set_menu_foot(foot);
    open_menu(h, rows, right, top);
    // The rows are built on the first frame; the panel is placed by its height.
    h.render();
    h.advance(0);
}

#[test]
fn a_dropdown_hangs_from_its_buttons_right_edge_and_opens_upward_4_px_clear() {
    for scale in SCALES {
        let h = harness(scale);
        // A button from 250 to 292 css x, 240 to 274 css y: room above, none below.
        open_drop(
            &h,
            vec![row("One"), row("Two"), row("Three")],
            292.0,
            240.0,
            274.0,
        );
        let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
        let (pw, ph) = (h.comp.get_panel_w(), h.comp.get_panel_h());
        let px1 = 1.0 / scale;
        assert!(
            (px + pw - 292.0).abs() <= px1,
            "right edge {} at {scale}",
            px + pw
        );
        assert!(px < 250.0, "it opens toward the left, from {px} at {scale}");
        assert!(
            (py + ph - 236.0).abs() <= px1,
            "foot {} at {scale}",
            py + ph
        );
        // The pixels agree: no panel right of the button's edge, nor between it and the button.
        let f = h.render();
        let right = h.phys(292.5);
        assert_eq!(
            f.count_where(right, 0, f.w, f.h, |p| Frame::near(p, SIDE_FLAT, 2)),
            0,
            "nothing right of the button at {scale}"
        );
        let below = h.phys(236.5);
        assert_eq!(
            f.count_where(0, below, f.w, f.h, |p| Frame::near(p, SIDE_FLAT, 2)),
            0,
            "nothing under its foot at {scale}"
        );
    }
}

#[test]
fn a_dropdown_drops_below_when_there_is_no_room_above_and_stays_in_the_window() {
    for scale in SCALES {
        let h = harness(scale);
        // A button near the top: three rows do not fit above it, so the panel hangs under its foot.
        open_drop(
            &h,
            vec![row("One"), row("Two"), row("Three")],
            292.0,
            30.0,
            64.0,
        );
        let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
        let pw = h.comp.get_panel_w();
        let px1 = 1.0 / scale;
        assert!((py - 68.0).abs() <= px1, "top {py} at {scale}");
        assert!((px + pw - 292.0).abs() <= px1, "right edge at {scale}");

        // A button at the window's left edge: the panel is held 8 px inside rather than leaving it.
        let h = harness(scale);
        open_drop(&h, vec![row("One")], 40.0, 240.0, 274.0);
        assert!(
            (h.comp.get_panel_x() - 8.0).abs() <= px1,
            "left edge {} at {scale}",
            h.comp.get_panel_x()
        );

        // Neither side has room (a very short window): the panel starts 8 px from the top.
        let h = harness(scale);
        let many: Vec<MenuRow> = (0..12).map(|i| row(&format!("Row {i}"))).collect();
        open_drop(&h, many, 292.0, 120.0, 154.0);
        assert!(
            (h.comp.get_panel_y() - 8.0).abs() <= px1,
            "top {} at {scale}",
            h.comp.get_panel_y()
        );
    }
}

#[test]
fn the_menu_clamps_inside_the_window() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, vec![row("One"), row("Two"), row("Three")], 390.0, 290.0);
        let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
        let (pw, ph) = (h.comp.get_panel_w(), h.comp.get_panel_h());
        assert!(px + pw <= 400.0 - 8.0 + 0.01, "right edge at {scale}");
        assert!(py + ph <= 300.0 - 8.0 + 0.01, "bottom edge at {scale}");
        // and the pixels agree: nothing of the panel is drawn past 392 css px
        let f = h.render();
        let from = h.phys(392.5);
        assert_eq!(
            f.count_where(from, 0, f.w, f.h, |p| Frame::near(p, SIDE_FLAT, 2)),
            0
        );
    }
}

#[test]
fn rows_are_28_tall_with_hairlines_between() {
    for scale in [1.0_f32, 2.25] {
        let h = harness(scale);
        open_menu(&h, vec![row("One"), row("Two"), row("Three")], 20.0, 20.0);
        let px = h.comp.get_panel_x();
        let py = h.comp.get_panel_y();
        let hair = (scale.floor().max(1.0)) / scale;
        // hover the first row, away from the label
        let hx = px + 3.0;
        let hy = py + hair + 14.0;
        h.move_to(hx, hy);
        h.advance(0);
        let f = h.render();
        let x = h.phys(hx);
        let y_top = h.phys(py + hair);
        let hover = f.px(x, y_top + 2);
        assert!(
            !Frame::near(hover, SIDE_FLAT, 1),
            "the hovered row differs from the plain panel"
        );
        let run = f.run_len_v(x, y_top, |p| Frame::near(p, hover, 1));
        assert_eq!(run, h.phys(28.0), "hover is 28 css px tall at {scale}");
        // two dividers: lines in the faint divider colour, hairline thick, inside the panel
        let divider = f.px(x, y_top + h.phys(28.0));
        assert!(
            !Frame::near(divider, SIDE_FLAT, 0),
            "a divider line below row one"
        );
        let dev_hair = scale.floor().max(1.0) as u32;
        let thick = f.run_len_v(x, y_top + h.phys(28.0), |p| Frame::near(p, divider, 1));
        assert_eq!(
            thick, dev_hair,
            "a hairline is {dev_hair} device px at {scale}"
        );
        let second = y_top + h.phys(28.0) + dev_hair + h.phys(28.0);
        assert!(
            !Frame::near(f.px(x, second), SIDE_FLAT, 0),
            "a divider below row two"
        );
    }
}

/// Task 27 (M5 screenshots): the last row was 2 css px taller than the others (the panel added
/// the border on top of a layout that already carries it as padding).
#[test]
fn every_row_is_28_tall_including_the_last_and_the_border_closes_it() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, vec![row("One"), row("Two"), row("Three")], 20.0, 20.0);
        let px = h.comp.get_panel_x();
        let py = h.comp.get_panel_y();
        let f = h.render();
        let x = h.phys(px + 3.0);
        let dev_hair = scale.floor().max(1.0) as u32;
        let mut y = h.phys(py) + dev_hair;
        for i in 0..3 {
            let run = f.run_len_v(x, y, |p| Frame::near(p, SIDE_FLAT, 1));
            assert_eq!(run, h.phys(28.0), "row {i} at {scale}");
            y += run;
            let line = f.run_len_v(x, y, |p| !Frame::near(p, SIDE_FLAT, 1));
            assert!(line >= dev_hair, "a line below row {i} at {scale}");
            y += dev_hair;
        }
        let want_h = 3.0 * 28.0 + 4.0 * dev_hair as f32 / scale;
        assert!(
            (h.comp.get_panel_h() - want_h).abs() < 0.01,
            "panel {} tall, want {want_h} at {scale}",
            h.comp.get_panel_h()
        );
    }
}

/// Parity line 43: the panel is at least 156 wide and ends on a whole device pixel, and its
/// metrics are the ones PT's ContextMenu sets (1 px divider border, 2 px radius, the shadow
/// 0 10px 28px rgba(0,0,0,.5)). The metrics are read from menus.slint, the source they live in.
#[test]
fn the_menu_panel_has_156_min_width_1px_border_2px_radius_and_the_shadow() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, vec![row("A")], 20.0, 20.0);
        let _ = h.render();
        let w = h.comp.get_panel_w();
        assert!(
            w >= 156.0 - 0.01,
            "a short menu is {w} css px wide at {scale}"
        );
        let dev = w * scale;
        assert!(
            (dev - dev.round()).abs() < 0.01,
            "the panel is {dev} device px wide at {scale}"
        );
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/menus.slint");
    let src = std::fs::read_to_string(path).expect("menus.slint"); // sync-ok: test code only
    for want in [
        "min(max(156px",
        "border-radius: 2px;",
        "border-width: Px.hair();",
        "drop-shadow-offset-y: 10px;",
        "drop-shadow-blur: 28px;",
        "drop-shadow-color: #00000080;",
    ] {
        assert!(src.contains(want), "menus.slint lost `{want}`");
    }
}

#[test]
fn a_pick_reports_the_row_and_asks_to_close() {
    let h = harness(1.0);
    let mut rows = vec![row("One"), row("Two"), row("Three")];
    rows[2].disabled = true;
    open_menu(&h, rows, 20.0, 20.0);
    let px = h.comp.get_panel_x();
    let py = h.comp.get_panel_y();
    // the second row: 1 border + 28 + 1 divider + 14
    h.click(px + 20.0, py + 1.0 + 28.0 + 1.0 + 14.0);
    assert_eq!(h.comp.get_picked_index(), 1);
    assert_eq!(h.comp.get_menu_dismissed(), 1);
    // a disabled row reports nothing
    h.comp.set_picked_index(-1);
    open_menu(
        &h,
        vec![row("One"), row("Two"), {
            let mut r = row("Three");
            r.disabled = true;
            r
        }],
        20.0,
        20.0,
    );
    h.comp.set_menu_dismissed(0);
    h.click(px + 20.0, py + 1.0 + 2.0 * 29.0 + 14.0);
    assert_eq!(h.comp.get_picked_index(), -1);
    assert_eq!(h.comp.get_menu_dismissed(), 0);
}

#[test]
fn a_trailing_click_toggles_without_picking_or_closing() {
    let h = harness(1.0);
    let mut rows = vec![row("One"), row("Two")];
    rows[0].trailing_icon = "M15 3l6 6-3 1-4 4 .5 4.5L10 14l-6 6-1-1 6-6-4.5-4.5L9 8l4-4z".into();
    rows[0].trailing_filled = false;
    open_menu(&h, rows.clone(), 20.0, 20.0);
    let plain = h.render();
    let px = h.comp.get_panel_x();
    let py = h.comp.get_panel_y();
    let pw = h.comp.get_panel_w();
    // the 20 x 20 button sits 11 px (padding) minus 4 (margin) from the panel's right inner edge
    let bx = px + pw - 1.0 - 11.0 + 4.0 - 10.0 + 0.0;
    let by = py + 1.0 + 14.0;
    h.click(bx, by);
    assert_eq!(h.comp.get_trailing_index(), 0);
    assert_eq!(h.comp.get_picked_index(), -1);
    assert_eq!(h.comp.get_menu_dismissed(), 0);
    // the glue flips the filled flag; the pin is drawn filled
    let mut filled = rows;
    filled[0].trailing_filled = true;
    h.comp.set_menu_rows(menu_rows(filled));
    h.move_to(390.0, 10.0);
    h.advance(0);
    let after = h.render();
    let (x0, y0) = (h.phys(bx - 8.0), h.phys(by - 8.0));
    let (x1, y1) = (h.phys(bx + 8.0), h.phys(by + 8.0));
    let ink = |p: [u8; 4]| p[0] > 0x60;
    assert!(
        after.count_where(x0, y0, x1, y1, ink) > plain.count_where(x0, y0, x1, y1, ink),
        "the filled pin has more ink than the outline"
    );
}

#[test]
fn a_press_outside_or_escape_dismisses() {
    let h = harness(1.0);
    open_menu(&h, vec![row("One"), row("Two")], 100.0, 100.0);
    h.click(5.0, 5.0);
    assert_eq!(h.comp.get_menu_dismissed(), 1);
    assert!(!h.comp.get_menu_open());
    open_menu(&h, vec![row("One"), row("Two")], 100.0, 100.0);
    h.key_text(slint::SharedString::from(slint::platform::Key::Escape).as_str());
    assert_eq!(h.comp.get_menu_dismissed(), 2);
}

#[test]
fn a_right_click_outside_reports_its_point_and_a_left_press_only_closes() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, vec![row("One"), row("Two")], 100.0, 100.0);
        // A right click outside: its point goes out, and the scrim itself closes nothing (the app does).
        h.right_click(300.0, 50.0);
        assert_eq!(h.comp.get_menu_outside_rights(), 1, "scale {scale}");
        assert!(
            (h.comp.get_menu_outside_x() - 300.0).abs() < 0.01,
            "scale {scale}"
        );
        assert!(
            (h.comp.get_menu_outside_y() - 50.0).abs() < 0.01,
            "scale {scale}"
        );
        assert_eq!(h.comp.get_menu_dismissed(), 0, "scale {scale}");
        // The same spot again reports again: it is never a toggle.
        h.right_click(300.0, 50.0);
        assert_eq!(h.comp.get_menu_outside_rights(), 2, "scale {scale}");
        // A right click inside the panel is unchanged: nothing reported, nothing closed.
        let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
        h.right_click(px + 20.0, py + 14.0);
        assert_eq!(h.comp.get_menu_outside_rights(), 2, "scale {scale}");
        assert_eq!(h.comp.get_menu_dismissed(), 0, "scale {scale}");
        assert!(h.comp.get_menu_open(), "scale {scale}");
        assert_eq!(h.comp.get_picked_index(), -1, "scale {scale}");
        // The spot the menu hangs from is the panel's corner: a right click there is the same click again.
        h.right_click(px + 1.0, py + 1.0);
        assert_eq!(h.comp.get_menu_outside_rights(), 3, "scale {scale}");
        // A left press outside only closes.
        h.click(300.0, 50.0);
        assert_eq!(h.comp.get_menu_dismissed(), 1, "scale {scale}");
        assert_eq!(h.comp.get_menu_outside_rights(), 3, "scale {scale}");
    }
}

#[test]
fn the_overflow_list_hands_on_a_right_click_outside_too() {
    for scale in SCALES {
        let h = harness(scale);
        open_list(&h, vec![orow("a", false), orow("b", false)]);
        h.right_click(5.0, 250.0);
        assert_eq!(h.comp.get_ov_outside_rights(), 1, "scale {scale}");
        assert!(
            (h.comp.get_ov_outside_x() - 5.0).abs() < 0.01,
            "scale {scale}"
        );
        assert!(
            (h.comp.get_ov_outside_y() - 250.0).abs() < 0.01,
            "scale {scale}"
        );
        assert_eq!(h.comp.get_ov_dismissed(), 0, "scale {scale}");
        h.click(5.0, 250.0);
        assert_eq!(h.comp.get_ov_dismissed(), 1, "scale {scale}");
        assert_eq!(h.comp.get_ov_outside_rights(), 1, "scale {scale}");
    }
}

#[test]
fn a_left_press_on_the_anchor_fringe_outside_the_panel_closes() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, vec![row("One"), row("Two")], 100.0, 100.0);
        // The anchor square reaches 2 px past the panel's corner: that is outside, so a left press closes.
        h.click(h.comp.get_panel_x() - 1.0, h.comp.get_panel_y() - 1.0);
        assert_eq!(h.comp.get_menu_dismissed(), 1, "scale {scale}");
        assert_eq!(h.comp.get_menu_outside_rights(), 0, "scale {scale}");
    }
}

#[test]
fn a_menu_replaced_in_one_turn_starts_without_a_flyout_and_escape_still_closes() {
    let scale = 1.0;
    let h = harness(scale);
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    hover_row(&h, scale, 0);
    assert!(h.comp.get_fly_open());
    // The reopen: the rows change and `open` never goes false in between, as in a hand-over.
    h.comp.set_menu_rows(menu_rows(flyout_rows()));
    h.comp.set_menu_x(60.0);
    h.comp.set_menu_y(60.0);
    h.comp.invoke_menu_shown();
    h.advance(0);
    assert!(!h.comp.get_fly_open(), "the old flyout index is gone");
    // The menu holds the keys: a real Escape reaches its FocusScope.
    h.key_text(slint::SharedString::from(slint::platform::Key::Escape).as_str());
    assert_eq!(h.comp.get_menu_dismissed(), 1);
}

#[test]
fn the_overflow_list_opens_under_the_anchor_and_focuses_the_field() {
    let h = harness(1.0);
    // a wider window so the clamp does not hide the rule: anchor-right 900 needs 900 css px
    drop(h);
    let h = Harness::new(1.0, 1000, 300, || MenusPreview::new().unwrap());
    h.comp
        .set_ov_rows(ModelRc::new(VecModel::from(vec![orow("t1", false)])));
    h.comp.set_ov_right(900.0);
    h.comp.set_ov_bottom(36.0);
    h.comp.set_ov_open(true);
    h.advance(0);
    assert_eq!(h.comp.get_ov_box_x(), 560.0);
    assert_eq!(h.comp.get_ov_box_y(), 40.0);
    assert_eq!(h.comp.get_ov_box_w(), 340.0);
    h.key_text("ap");
    assert_eq!(h.comp.get_ov_edited().as_str(), "ap");
    assert_eq!(h.comp.get_ov_query().as_str(), "ap");
}

#[test]
fn the_overflow_list_orders_needs_first_and_has_headings() {
    let h = harness(1.0);
    let rows = vec![
        orow("t1", true),
        orow("t2", true),
        orow("t3", false),
        orow("t4", false),
        orow("t5", false),
    ];
    open_list(&h, rows);
    let heads: Vec<String> = (0..5)
        .map(|i| h.comp.invoke_ov_heading_for(i).to_string())
        .collect();
    assert_eq!(heads, ["Needs you", "", "Other tabs", "", ""]);
    // without needs rows the heading is Tabs
    h.comp.set_ov_rows(ModelRc::new(VecModel::from(vec![
        orow("a", false),
        orow("b", false),
    ])));
    h.advance(0);
    assert_eq!(h.comp.invoke_ov_heading_for(0).as_str(), "Tabs");
    assert_eq!(h.comp.invoke_ov_heading_for(1).as_str(), "");
}

#[test]
fn enter_accepts_and_a_row_click_picks() {
    let h = Harness::new(1.0, 1000, 300, || MenusPreview::new().unwrap());
    let rows: Vec<OverflowRow> = ["t1", "t2", "t3", "t4"]
        .iter()
        .map(|i| orow(i, false))
        .collect();
    h.comp.set_ov_rows(ModelRc::new(VecModel::from(rows)));
    h.comp.set_ov_right(900.0);
    h.comp.set_ov_bottom(36.0);
    h.comp.set_ov_open(true);
    h.advance(0);
    h.key_text("\n");
    assert_eq!(h.comp.get_ov_accepted(), 1);
    // the third row: head, 4 padding, one heading, two rows, half a row
    let x = h.comp.get_ov_box_x() + 100.0;
    let y = h.comp.get_ov_box_y()
        + h.comp.get_ov_head_h()
        + h.comp.get_ov_body_pad()
        + h.comp.get_ov_heading_h()
        + 2.0 * h.comp.get_ov_row_h()
        + 17.0;
    h.click(x, y);
    assert_eq!(h.comp.get_ov_picked().as_str(), "t3");
}

#[test]
fn an_empty_list_says_so() {
    let h = harness(1.0);
    open_list(&h, vec![]);
    let f = h.render();
    let x0 = h.phys(h.comp.get_ov_box_x());
    let y0 = h.phys(h.comp.get_ov_box_y() + h.comp.get_ov_head_h() + 2.0);
    let ink = f.count_where(x0, y0, x0 + h.phys(300.0), y0 + h.phys(40.0), |p| {
        !Frame::near(p, SIDE_FLAT, 8)
    });
    assert!(ink > 20, "the message is drawn ({ink} ink pixels)");
    assert_eq!(h.comp.get_ov_rows().row_count(), 0);
}

#[test]
fn the_tip_reports_its_size_and_wraps_at_320() {
    let h = harness(1.0);
    h.comp.set_tip_text("word ".repeat(60).trim().into());
    h.comp.set_tip_visible(true);
    h.advance(200);
    assert!(
        h.comp.get_tip_w() <= 320.0 + 16.0,
        "w {}",
        h.comp.get_tip_w()
    );
    assert!(h.comp.get_tip_h() > 32.0, "h {}", h.comp.get_tip_h());
    h.comp.set_tip_text("Short".into());
    h.advance(200);
    assert!(
        (h.comp.get_tip_h() - 26.0).abs() < 6.0,
        "h {}",
        h.comp.get_tip_h()
    );
}

#[test]
fn the_tip_is_pointer_transparent_and_hidden_when_empty() {
    let h = harness(1.0);
    h.comp.set_tip_text("Hello".into());
    h.comp.set_tip_visible(false);
    h.advance(300);
    let f = h.render();
    let bg = f.px(f.w - 1, f.h - 1);
    assert_eq!(
        f.count_where(0, 0, f.w, f.h, |p| !Frame::near(p, bg, 0)),
        0,
        "nothing is drawn while hidden"
    );
    // visible: a click on the bubble still reaches the touch area behind
    h.comp.set_tip_visible(true);
    h.advance(300);
    h.click(30.0, 30.0);
    assert_eq!(h.comp.get_back_clicks(), 1);
}

#[test]
fn a_label_is_never_cut_for_its_key_hint() {
    // Visual review (Task 33): the tab menu read "Clo..." beside Ctrl+W. PT's panel is at
    // least 156 wide and grows to its rows; the label never gives way to the hint.
    let labelled = |hint: &str| MenuRow {
        icon: "M7 7l10 10M17 7L7 17".into(),
        hint: hint.into(),
        ..row("Copy folder path")
    };
    for scale in [1.0_f32, 1.5, 2.25] {
        let h = harness(scale);
        // Widths are read after a paint: the rows' repeater is laid out by it.
        open_menu(&h, vec![labelled("")], 20.0, 20.0);
        let bare = h.render();
        let bare_w = h.comp.get_panel_w();
        open_menu(&h, vec![labelled("Ctrl+Shift+C")], 20.0, 20.0);
        let hinted = h.render();
        let hinted_w = h.comp.get_panel_w();
        assert!(
            bare_w > 160.0,
            "scale {scale}: the label alone fits in {bare_w}"
        );
        // A panel that grows to its text still ends on a whole device pixel (crisp border).
        for (what, w) in [("bare", bare_w), ("hinted", hinted_w)] {
            let dev = w * scale;
            assert!(
                (dev - dev.round()).abs() < 0.01,
                "scale {scale}: the {what} panel is {dev} device px wide"
            );
        }
        assert!(
            hinted_w >= bare_w + 40.0,
            "scale {scale}: the hint did not widen the panel ({bare_w} to {hinted_w})"
        );
        // The label is drawn the same, pixel for pixel, with the hint beside it.
        let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
        let (x0, x1) = (h.phys(px + 2.0), h.phys(px + bare_w - 34.0));
        let (y0, y1) = (h.phys(py + 2.0), h.phys(py + 28.0));
        for y in y0..y1 {
            for x in x0..x1 {
                assert_eq!(
                    bare.px(x, y),
                    hinted.px(x, y),
                    "scale {scale}: the label differs at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn a_menu_opens_on_the_pointers_device_pixel_at_every_scale() {
    // Audit visual review (V10): the tab menu was drawn one device pixel below PT's frame. The
    // pointer is a whole device pixel; the panel's top lands on that same row (Px.snap rounds to
    // the device grid), so a pointer on device row N draws the border on row N at every scale.
    for scale in SCALES {
        let h = harness(scale);
        for dev in [100.0_f32, 101.0, 102.0] {
            open_menu(&h, vec![row("Close tab")], 20.0, dev / scale);
            let top = h.comp.get_panel_y() * scale;
            assert!(
                (top - dev).abs() < 0.01,
                "scale {scale}: a pointer on device row {dev} drew the panel at {top}"
            );
        }
    }
}

// ---- Parity line 44: one flyout level, keepOpen rows (PT ContextMenu.tsx) ----

/// Row 0 "Open with" carries two children (flat rows 3 and 4, `parent: 0`); rows 1 and 2 are plain.
fn flyout_rows() -> Vec<MenuRow> {
    let mut a = row("Open with");
    a.flyout = true;
    let mut x = row("Notepad");
    x.parent = 0;
    let mut y = row("Paint");
    y.parent = 0;
    vec![a, row("Two"), row("Three"), x, y]
}

/// One css hairline at this scale (the divider and border thickness the menu draws).
fn hair(scale: f32) -> f32 {
    scale.floor().max(1.0) / scale
}

/// The middle of top-level row `i` of the open panel.
fn top_row(h: &Harness<MenusPreview>, scale: f32, i: usize) -> (f32, f32) {
    let hr = hair(scale);
    (
        h.comp.get_panel_x() + 20.0,
        h.comp.get_panel_y() + hr + i as f32 * (28.0 + hr) + 14.0,
    )
}

/// The middle of flyout row `i`.
fn fly_row(h: &Harness<MenusPreview>, scale: f32, i: usize) -> (f32, f32) {
    let hr = hair(scale);
    (
        h.comp.get_fly_x() + 20.0,
        h.comp.get_fly_y() + hr + i as f32 * (28.0 + hr) + 14.0,
    )
}

fn hover_row(h: &Harness<MenusPreview>, scale: f32, i: usize) {
    let (x, y) = top_row(h, scale, i);
    h.move_to(x, y);
    h.advance(0);
}

#[test]
fn a_flyout_row_opens_one_panel_beside_it_overlapping_6_px_and_level_with_the_row() {
    for scale in SCALES {
        let h = harness(scale);
        open_menu(&h, flyout_rows(), 20.0, 20.0);
        let _ = h.render();
        assert!(
            !h.comp.get_fly_open(),
            "no flyout before a hover at {scale}"
        );
        hover_row(&h, scale, 0);
        let f = h.render();
        assert!(
            h.comp.get_fly_open(),
            "hovering the row opens it at {scale}"
        );
        let tol = 1.0 / scale + 0.01;
        let (px, pw) = (h.comp.get_panel_x(), h.comp.get_panel_w());
        assert!(
            (h.comp.get_fly_x() - (px + pw - 6.0)).abs() <= tol,
            "the flyout overlaps the panel by 6 px at {scale}: fly {} panel right {}",
            h.comp.get_fly_x(),
            px + pw
        );
        // Its first row meets the parent row's top (the parent row starts one hairline in).
        let row_top = h.comp.get_panel_y() + hair(scale);
        assert!(
            (h.comp.get_fly_y() + hair(scale) - row_top).abs() <= tol,
            "the flyout is level with its row at {scale}"
        );
        let fw = h.comp.get_fly_w();
        assert!(
            (176.0 - 0.01..=260.0 + 0.01).contains(&fw),
            "flyout {fw} wide"
        );
        assert!(
            ((fw * scale) - (fw * scale).round()).abs() < 0.01,
            "flyout ends on a whole device pixel at {scale}"
        );
        // Two children, 28 tall each, hairlines between and around.
        let want_h = 2.0 * 28.0 + 3.0 * hair(scale);
        assert!(
            (h.comp.get_fly_h() - want_h).abs() < 0.01,
            "flyout {} tall, want {want_h} at {scale}",
            h.comp.get_fly_h()
        );
        // It is drawn: a point inside it, past the parent panel, is the panel's flat colour.
        let (ix, iy) = (
            h.phys(h.comp.get_fly_x() + fw - 10.0),
            h.phys(h.comp.get_fly_y() + 20.0),
        );
        assert!(
            Frame::near(f.px(ix, iy), SIDE_FLAT, 2),
            "flyout drawn at {scale}"
        );
    }
}

#[test]
fn the_flyout_flips_left_at_the_right_edge() {
    for scale in SCALES {
        let h = harness(scale);
        // Asked far right: the panel clamps to the window, so the flyout has no room on the right.
        open_menu(&h, flyout_rows(), 390.0, 20.0);
        hover_row(&h, scale, 0);
        let _ = h.render();
        assert!(h.comp.get_fly_open());
        let tol = 1.0 / scale + 0.01;
        let want = h.comp.get_panel_x() - h.comp.get_fly_w() + 6.0;
        assert!(
            (h.comp.get_fly_x() - want).abs() <= tol,
            "flipped left at {scale}: fly {} want {want}",
            h.comp.get_fly_x()
        );
        assert!(
            h.comp.get_fly_x() >= 8.0 - 0.01,
            "and still inside the window"
        );
    }
}

#[test]
fn the_flyout_never_runs_off_the_bottom_or_top() {
    for scale in SCALES {
        let h = harness(scale);
        let mut rows = flyout_rows();
        for n in 0..4 {
            let mut c = row(&format!("Child {n}"));
            c.parent = 0;
            rows.push(c);
        }
        open_menu(&h, rows, 20.0, 150.0);
        hover_row(&h, scale, 0);
        let _ = h.render();
        assert!(h.comp.get_fly_open());
        assert!(
            h.comp.get_fly_y() + h.comp.get_fly_h() <= 300.0 - 8.0 + 0.01,
            "bottom at {scale}: {} + {}",
            h.comp.get_fly_y(),
            h.comp.get_fly_h()
        );
        assert!(h.comp.get_fly_y() >= 8.0 - 0.01, "top at {scale}");
    }
}

#[test]
fn leaving_a_flyout_row_gives_a_grace_period_and_reaching_the_flyout_cancels_the_close() {
    let scale = 1.0;
    let h = harness(scale);
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    hover_row(&h, scale, 0);
    assert!(h.comp.get_fly_open());
    // The diagonal path to the flyout crosses the rows below: row 1 does not close it at once.
    hover_row(&h, scale, 1);
    h.advance(150);
    assert!(
        h.comp.get_fly_open(),
        "still open 150 ms after leaving the row"
    );
    // Reaching the flyout inside the grace period cancels the close for good.
    let (fx, fy) = fly_row(&h, scale, 0);
    h.move_to(fx, fy);
    h.advance(0);
    h.advance(600);
    assert!(
        h.comp.get_fly_open(),
        "the pointer in the flyout keeps it up"
    );
    // Back to a plain row and staying there closes it once the grace period is over.
    hover_row(&h, scale, 2);
    h.advance(200);
    assert!(h.comp.get_fly_open(), "200 ms is inside the 300 ms grace");
    h.advance(150);
    assert!(!h.comp.get_fly_open(), "closed after the grace period");
    // The parent row again opens it, and coming back to the parent cancels a pending close.
    hover_row(&h, scale, 0);
    assert!(h.comp.get_fly_open());
    hover_row(&h, scale, 1);
    h.advance(100);
    hover_row(&h, scale, 0);
    h.advance(600);
    assert!(
        h.comp.get_fly_open(),
        "back on the parent cancels the close"
    );
}

#[test]
fn a_keep_open_row_acts_and_leaves_the_menu_and_the_flyout_up() {
    for scale in [1.0_f32, 2.25] {
        let h = harness(scale);
        let mut rows = flyout_rows();
        rows[3].keep_open = true; // Notepad, in the flyout
        rows[1].keep_open = true; // Two, on the panel
        open_menu(&h, rows, 20.0, 20.0);
        hover_row(&h, scale, 0);
        let (fx, fy) = fly_row(&h, scale, 0);
        h.click(fx, fy);
        assert_eq!(
            h.comp.get_picked_index(),
            3,
            "the flyout row reports its flat index"
        );
        assert_eq!(
            h.comp.get_menu_dismissed(),
            0,
            "keepOpen: not dismissed at {scale}"
        );
        assert!(h.comp.get_menu_open() && h.comp.get_fly_open());
        // A second pick (undoing a tick) still works while it stands.
        h.comp.set_picked_index(-1);
        h.click(fx, fy);
        assert_eq!(h.comp.get_picked_index(), 3);
        // A plain flyout row picks and closes the whole menu.
        let (fx2, fy2) = fly_row(&h, scale, 1);
        h.click(fx2, fy2);
        assert_eq!(h.comp.get_picked_index(), 4);
        assert_eq!(
            h.comp.get_menu_dismissed(),
            1,
            "an ordinary row closes at {scale}"
        );
        // A keepOpen row on the panel itself also keeps it up.
        open_menu(
            &h,
            {
                let mut r = flyout_rows();
                r[1].keep_open = true;
                r
            },
            20.0,
            20.0,
        );
        h.comp.set_menu_dismissed(0);
        let (x, y) = top_row(&h, scale, 1);
        h.click(x, y);
        assert_eq!(h.comp.get_picked_index(), 1);
        assert_eq!(h.comp.get_menu_dismissed(), 0);
        assert!(h.comp.get_menu_open());
    }
}

#[test]
fn a_flyout_row_is_flyout_only_unless_it_brings_its_own_pick() {
    let scale = 1.0;
    let h = harness(scale);
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    let (x, y) = top_row(&h, scale, 0);
    h.click(x, y);
    assert_eq!(
        h.comp.get_picked_index(),
        -1,
        "no pick from a flyout-only parent"
    );
    assert_eq!(h.comp.get_menu_dismissed(), 0);
    assert!(
        h.comp.get_menu_open() && h.comp.get_fly_open(),
        "the click leaves the flyout up"
    );
    let mut rows = flyout_rows();
    rows[0].flyout_pick = true;
    open_menu(&h, rows, 20.0, 20.0);
    h.click(x, y);
    assert_eq!(
        h.comp.get_picked_index(),
        0,
        "the parent picks when it brings its own"
    );
    assert_eq!(h.comp.get_menu_dismissed(), 1);
}

#[test]
fn there_is_one_flyout_level_only() {
    let scale = 1.0;
    let h = harness(scale);
    let mut rows = flyout_rows();
    rows[3].flyout = true; // a child that claims a flyout of its own
    open_menu(&h, rows, 20.0, 20.0);
    hover_row(&h, scale, 0);
    let (fx, fy) = fly_row(&h, scale, 0);
    h.move_to(fx, fy);
    h.advance(600);
    assert!(h.comp.get_fly_open(), "the first level stays");
    assert_eq!(
        h.comp.get_open_sub(),
        0,
        "no second level replaces or nests it"
    );
}

#[test]
fn escape_closes_the_whole_menu_and_a_fresh_open_starts_without_a_flyout() {
    let scale = 1.0;
    let h = harness(scale);
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    hover_row(&h, scale, 0);
    assert!(h.comp.get_fly_open());
    h.key_text(slint::SharedString::from(slint::platform::Key::Escape).as_str());
    assert_eq!(h.comp.get_menu_dismissed(), 1);
    assert!(!h.comp.get_menu_open());
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    assert!(!h.comp.get_fly_open(), "a reopened menu has no flyout up");
}

#[test]
fn a_press_on_the_flyout_does_not_dismiss_and_one_outside_both_does() {
    let scale = 1.0;
    let h = harness(scale);
    open_menu(&h, flyout_rows(), 20.0, 20.0);
    hover_row(&h, scale, 0);
    // The hairline between the two flyout rows is panel, not a row: still not "outside".
    let hr = hair(scale);
    h.click(
        h.comp.get_fly_x() + 20.0,
        h.comp.get_fly_y() + hr + 28.0 + hr / 2.0,
    );
    assert_eq!(h.comp.get_menu_dismissed(), 0);
    h.click(390.0, 290.0);
    assert_eq!(h.comp.get_menu_dismissed(), 1);
}
