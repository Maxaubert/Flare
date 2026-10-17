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
