//! Component tests for the tab sidebar (Task 21), headless: pointer events, the platform clock and
//! pixel measuring at every display scaling.

use std::cell::RefCell;
use std::rc::Rc;

use flare_tabs::keymap::{ChordId, key_label};
use slint::platform::{Key, PointerEventButton};
use slint::{ModelRc, SharedString, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{TabRow, TabSidebarPreview};

const NAMES: [&str; 6] = ["api", "web", "docs", "site", "tools", "notes"];
/// `Theme.secondary` (the panel) and `Theme.ground` (the page behind it).
const PANEL: [u8; 4] = [13, 13, 13, 255];
const GROUND: [u8; 4] = [23, 23, 23, 255];

fn rows(active: usize) -> Vec<TabRow> {
    NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| TabRow {
            id: format!("t{i}").into(),
            label: (*name).into(),
            tip: format!("C:\\work\\{name}").into(),
            folder: format!("~\\work\\{name}").into(),
            active: i == active,
            kept: true,
            ..TabRow::default()
        })
        .collect()
}

fn harness(scale: f32) -> Harness<TabSidebarPreview> {
    let h = Harness::new(scale, 700, 500, || {
        TabSidebarPreview::new().expect("the preview builds")
    });
    h.comp.set_rows(ModelRc::new(VecModel::from(rows(1))));
    h.comp
        .set_find_key(key_label(ChordId::FindTab, true).into());
    h.advance(0);
    h
}

fn is_panel(p: [u8; 4]) -> bool {
    Frame::near(p, PANEL, 2)
}

/// The device pixel of a css length, rounded.
fn dev(h: &Harness<TabSidebarPreview>, css: f32) -> u32 {
    h.phys(css)
}

#[test]
fn a_row_is_46_tall_8_in_from_the_edges_with_4_between() {
    for scale in SCALES {
        let h = harness(scale);
        let f = h.render();
        let tol = 2_i64;
        // Row 3's card: css y 200..246, x 8..212 (panel 220 wide, rows area from y 50).
        let y_mid = dev(&h, 223.0);
        let x_left = (0..f.w)
            .find(|&x| !is_panel(f.px(x, y_mid)))
            .expect("a card is drawn");
        let want_left = i64::from(dev(&h, 8.0));
        assert!(
            (i64::from(x_left) - want_left).abs() <= tol,
            "scale {scale}: left {x_left} want {want_left}"
        );
        let x_right = (x_left..dev(&h, 219.0))
            .rev()
            .find(|&x| !is_panel(f.px(x, y_mid)))
            .expect("a card end");
        let want_right = i64::from(dev(&h, 212.0));
        assert!(
            (i64::from(x_right) + 1 - want_right).abs() <= tol,
            "scale {scale}: right {} want {want_right}",
            x_right + 1
        );
        // The card's height and the gap, down a column that holds no glyph (css x 207).
        let x_col = dev(&h, 150.0);
        let y0 = dev(&h, 197.0);
        let top = (y0..dev(&h, 260.0))
            .find(|&y| !is_panel(f.px(x_col, y)))
            .expect("the card top");
        let bottom = top + f.run_len_v(x_col, top, |p| !is_panel(p));
        let height = i64::from(bottom - top);
        let want_h = i64::from(dev(&h, 46.0));
        assert!(
            (height - want_h).abs() <= tol + 1,
            "scale {scale}: height {height} want {want_h}"
        );
        let gap = i64::from(f.run_len_v(x_col, bottom, is_panel));
        let want_gap = i64::from(dev(&h, 4.0));
        assert!(
            (gap - want_gap).abs() <= tol,
            "scale {scale}: gap {gap} want {want_gap}"
        );
    }
}

#[test]
fn the_active_row_is_tinted_with_the_accent_and_the_icon_band_is_square() {
    let h = harness(1.0);
    let f = h.render();
    // Row 1 is in front: css y 100..146. Its mid pixel at css x 207 holds no glyph.
    let p = f.px(207, 123);
    let want = [46_u8, 34, 24, 255];
    assert!(Frame::near(p, want, 3), "active tint {p:?} want {want:?}");
    // The inactive card beside it is the plain 3% wash.
    let q = f.px(207, 73);
    assert!(Frame::near(q, [19, 19, 19, 255], 3), "inactive {q:?}");
    // The band of row 0 (css y 50..96, x 8..54): a shade lighter than the card, 46 x 46.
    let band = |p: [u8; 4]| (25..=34).contains(&p[0]) && Frame::near(p, [p[0], p[0], p[0], 255], 2);
    let w = f.run_len_h(60, 20, band) + 12;
    assert!((i64::from(w) - 46).abs() <= 2, "band width {w}");
    let top = (50..70).find(|&y| band(f.px(20, y))).expect("band top");
    let hgt = f.run_len_v(20, top, band);
    assert!(
        (i64::from(hgt) - 46).abs() <= 4,
        "band height {hgt} (corner rounding shaves at most a few px)"
    );
}

#[test]
fn the_panel_width_is_what_it_is_told_and_snaps() {
    for scale in SCALES {
        let h = harness(scale);
        // What the glue passes: Px.snap(235px), a whole number of device pixels (294 at 1.25).
        let want = (235.0 * scale).round();
        h.comp.set_side_width(want / scale);
        h.advance(0);
        let f = h.render();
        let y = dev(&h, 420.0);
        assert_eq!(f.run_len_h(y, 0, is_panel), want as u32, "scale {scale}");
        assert!(
            Frame::near(f.px(want as u32, y), GROUND, 2),
            "scale {scale}: the page begins at device x {want}"
        );
    }
}

#[test]
fn collapsed_slots_take_no_room_and_slide_in_on_reveal() {
    let h = harness(1.0);
    h.comp.set_collapsed(true);
    h.advance(0);
    assert_eq!(h.comp.get_slot_width(), 0.0);
    h.advance(400);
    assert!(
        (h.comp.get_panel_left() + 230.0).abs() < 0.5,
        "waits 230 off the edge"
    );
    let f = h.render();
    assert_eq!(
        f.count_where(0, 0, f.w, f.h, |p| !Frame::near(p, GROUND, 1)),
        0,
        "nothing of it is on screen"
    );
    h.comp.set_revealed(true);
    h.advance(0);
    assert!(h.comp.get_panel_left() < -200.0, "still off at t = 0");
    h.advance(120);
    let mid = h.comp.get_panel_left();
    assert!(mid > -230.0 && mid < 0.0, "partly out at 120 ms: {mid}");
    h.advance(240);
    assert!(h.comp.get_panel_left().abs() < 0.5);
    let f = h.render();
    assert!(
        f.count_where(8, 50, 212, 340, |p| !is_panel(p)
            && !Frame::near(p, GROUND, 1))
            > 100,
        "the first rows are on screen"
    );
    h.comp.set_revealed(false);
    h.advance(0);
    h.advance(240);
    assert!((h.comp.get_panel_left() + 230.0).abs() < 0.5, "slides back");
}

#[test]
fn pinning_animates_the_slot_when_sliding() {
    let h = harness(1.0);
    h.comp.set_sliding(true);
    h.advance(0);
    assert!((h.comp.get_slot_visual() - 220.0).abs() < 0.5);
    h.comp.set_collapsed(true);
    assert_eq!(h.comp.get_slot_width(), 0.0, "the target changes at once");
    h.advance(0);
    assert!(h.comp.get_slot_visual() > 200.0, "still about 220 at t = 0");
    h.advance(120);
    let mid = h.comp.get_slot_visual();
    assert!(mid > 0.0 && mid < 220.0, "easing at 120 ms: {mid}");
    h.advance(240);
    assert!(h.comp.get_slot_visual().abs() < 0.5);
}

#[test]
fn the_find_box_filters_and_reports() {
    let h = harness(1.0);
    let edits = Rc::new(RefCell::new(Vec::<String>::new()));
    let accepted = Rc::new(RefCell::new(0));
    let escaped = Rc::new(RefCell::new(0));
    {
        let e = edits.clone();
        h.comp
            .on_find_edited(move |t| e.borrow_mut().push(t.to_string()));
        let a = accepted.clone();
        h.comp.on_find_accepted(move || *a.borrow_mut() += 1);
        let s = escaped.clone();
        h.comp.on_find_escape(move || *s.borrow_mut() += 1);
    }
    let ink = |p: [u8; 4]| p[0] > 60;
    // Empty: the placeholder (left) and the key label (right) are drawn.
    let f = h.render();
    assert!(f.count_where(40, 14, 110, 38, ink) > 0, "placeholder");
    assert!(f.count_where(140, 14, 204, 38, ink) > 0, "key label");
    h.click(100.0, 26.0);
    h.key_text("ap");
    assert_eq!(edits.borrow().last().map(String::as_str), Some("ap"));
    assert_eq!(h.comp.get_find_text(), "ap");
    assert!(h.comp.get_find_has_focus());
    let f = h.render();
    assert_eq!(
        f.count_where(140, 14, 204, 38, ink),
        0,
        "the key label is gone"
    );
    let (_, _, max_x, _) = f.bbox_where(40, 14, 204, 38, ink).expect("typed text");
    assert!(
        max_x < 75,
        "only the typed text is left, not the placeholder ({max_x})"
    );
    h.key_text("\n");
    assert_eq!(*accepted.borrow(), 1);
    let esc: SharedString = Key::Escape.into();
    h.key_text(&esc);
    assert_eq!(*escaped.borrow(), 1);
}

#[test]
fn a_press_reports_y_and_the_lanes_geometry() {
    let h = harness(1.0);
    let got = Rc::new(RefCell::new(Vec::<(String, i32, f32, f32, f32, f32)>::new()));
    let g = got.clone();
    h.comp.on_press(move |id, i, y, first, top, bottom| {
        g.borrow_mut()
            .push((id.to_string(), i, y, first, top, bottom));
    });
    let ups = Rc::new(RefCell::new(0));
    let u = ups.clone();
    h.comp.on_drag_released(move || *u.borrow_mut() += 1);
    h.move_to(100.0, 223.0);
    h.press(100.0, 223.0, PointerEventButton::Left);
    h.release(100.0, 223.0, PointerEventButton::Left);
    let got = got.borrow();
    assert_eq!(got.len(), 1);
    let (id, i, y, first, top, bottom) = got[0].clone();
    assert_eq!((id.as_str(), i), ("t3", 3));
    assert!((y - 223.0).abs() < 0.6, "pointer y {y}");
    assert!((first - 50.0).abs() < 0.6, "first row top {first}");
    assert!((top - 50.0).abs() < 0.6, "rows area top {top}");
    assert!((bottom - 449.0).abs() < 1.1, "rows area bottom {bottom}");
    assert_eq!(*ups.borrow(), 1);
}

#[test]
fn a_move_with_the_button_down_reports_the_pointer_y() {
    let h = harness(1.0);
    let moved = Rc::new(RefCell::new(Vec::<f32>::new()));
    let m = moved.clone();
    h.comp.on_drag_moved(move |y| m.borrow_mut().push(y));
    h.drag((100.0, 123.0), (100.0, 163.0), 4);
    let last = *moved.borrow().last().expect("moves were reported");
    assert!((last - 163.0).abs() < 0.6, "last y {last}");
}

#[test]
fn middle_click_closes_right_click_menus() {
    let h = harness(1.0);
    let closed = Rc::new(RefCell::new(Vec::<String>::new()));
    let menus = Rc::new(RefCell::new(Vec::<(String, f32, f32)>::new()));
    let pressed = Rc::new(RefCell::new(0));
    {
        let c = closed.clone();
        h.comp
            .on_closed(move |id| c.borrow_mut().push(id.to_string()));
        let m = menus.clone();
        h.comp
            .on_tab_menu(move |id, x, y| m.borrow_mut().push((id.to_string(), x, y)));
        let p = pressed.clone();
        h.comp
            .on_press(move |_, _, _, _, _, _| *p.borrow_mut() += 1);
    }
    h.middle_click(100.0, 173.0);
    assert_eq!(closed.borrow().as_slice(), ["t2".to_string()]);
    h.right_click(100.0, 173.0);
    let menus = menus.borrow();
    assert_eq!(menus.len(), 1);
    assert_eq!(menus[0].0, "t2");
    assert!((menus[0].1 - 100.0).abs() < 0.6 && (menus[0].2 - 173.0).abs() < 0.6);
    assert_eq!(*pressed.borrow(), 0, "neither starts a carry");
}

#[test]
fn a_click_on_the_x_closes_and_never_starts_a_carry() {
    let h = harness(1.0);
    let closed = Rc::new(RefCell::new(Vec::<String>::new()));
    let pressed = Rc::new(RefCell::new(0));
    {
        let c = closed.clone();
        h.comp
            .on_closed(move |id| c.borrow_mut().push(id.to_string()));
        let p = pressed.clone();
        h.comp
            .on_press(move |_, _, _, _, _, _| *p.borrow_mut() += 1);
    }
    // Row 1 is active, so its X is showing: css x 186..204, y 100 + 14.
    h.click(195.0, 123.0);
    assert_eq!(closed.borrow().as_slice(), ["t1".to_string()]);
    assert_eq!(*pressed.borrow(), 0);
}

#[test]
fn the_grip_drags_the_width_and_shows_its_line_only_on_hover() {
    for scale in SCALES {
        let h = harness(scale);
        let line = [153_u8, 102, 60, 255];
        let line_x = dev(&h, 220.0) - dev(&h, 3.0).max(1) + dev(&h, 2.0);
        let y = dev(&h, 300.0);
        let f = h.render();
        assert!(
            is_panel(f.px(line_x, y)),
            "scale {scale}: no line before hover"
        );
        h.move_to(219.5, 300.0);
        h.advance(200);
        let f = h.render();
        let run = f.run_len_h(y, line_x, |p| Frame::near(p, line, 10));
        let want = scale.floor().max(1.0) as u32;
        assert_eq!(
            run, want,
            "scale {scale}: the hover line is {want} device px"
        );
        assert!(
            !Frame::near(f.px(line_x - 1, y), line, 10),
            "scale {scale}: crisp on its left"
        );
    }
    let h = harness(1.0);
    let moved = Rc::new(RefCell::new(Vec::<f32>::new()));
    let released = Rc::new(RefCell::new(Vec::<f32>::new()));
    {
        let m = moved.clone();
        h.comp.on_grip_moved(move |w| m.borrow_mut().push(w));
        let r = released.clone();
        h.comp.on_grip_released(move |w| r.borrow_mut().push(w));
    }
    h.drag((220.0, 300.0), (250.0, 300.0), 3);
    let last = *moved.borrow().last().expect("grip moves were reported");
    assert!((last - 250.0).abs() < 0.6, "proposed width {last}");
    assert_eq!(released.borrow().len(), 1);
    assert!((released.borrow()[0] - 250.0).abs() < 0.6);
}

#[test]
fn the_new_tab_button_clicks_and_right_clicks() {
    let h = harness(1.0);
    let news = Rc::new(RefCell::new(0));
    let plus = Rc::new(RefCell::new(0));
    {
        let n = news.clone();
        h.comp.on_new_tab(move || *n.borrow_mut() += 1);
        let p = plus.clone();
        h.comp.on_plus_menu(move |_, _| *p.borrow_mut() += 1);
    }
    // Foot: from css y 449; the button is 34 high, 9 below it (8 padding and the hairline).
    h.move_to(20.0, 475.0);
    h.advance(0);
    let f = h.render();
    assert!(
        Frame::near(f.px(20, 475), [40, 40, 40, 255], 4),
        "hover bg {:?}",
        f.px(20, 475)
    );
    h.click(20.0, 475.0);
    assert_eq!(*news.borrow(), 1);
    h.right_click(20.0, 475.0);
    assert_eq!(*plus.borrow(), 1);
}

#[test]
fn the_collapsed_corner_and_line_are_drawn_hairline_thin() {
    for scale in SCALES {
        let h = harness(scale);
        h.comp.set_collapsed(true);
        h.comp.set_revealed(true);
        h.advance(0);
        h.advance(400);
        let f = h.render();
        let hair = scale.floor().max(1.0) as u32;
        let right = dev(&h, 220.0);
        let div = |p: [u8; 4]| Frame::near(p, [30, 30, 30, 255], 3);
        let y = dev(&h, 300.0);
        assert_eq!(f.run_len_h(y, right - hair, div), hair, "scale {scale}");
        assert!(
            !div(f.px(right - hair - 1, y)),
            "scale {scale}: crisp left of the line"
        );
        // The line starts below the corner (css y 10) and runs to the bottom.
        let top = dev(&h, 10.0);
        let col = f.run_len_v(right - 1, top + 2, div);
        assert!(
            col + top + 2 >= f.h - dev(&h, 56.0),
            "scale {scale}: runs down to the foot ({col})"
        );
        assert!(
            div(f.px(right - 1, f.h - 3)),
            "scale {scale}: and on below it"
        );
        // The arc of the corner: divider-coloured pixels round (css 223, 3).
        let near_arc = f.count_where(
            dev(&h, 221.0),
            dev(&h, 1.0),
            dev(&h, 226.0),
            dev(&h, 6.0),
            |p| Frame::near(p, [39, 39, 39, 255], 12) && !Frame::near(p, GROUND, 1),
        );
        assert!(near_arc > 0, "scale {scale}: the arc is drawn");
    }
}

/// A frame caught while the panel slides: its edge must sit on a whole device pixel, or the
/// panel's and the corner's antialiased edges meet in one pixel and show a seam (the e2e
/// verticalTabs "corner keeps to the sidebar's edge" read 4 px short on such a frame).
#[test]
fn the_sliding_panel_keeps_to_whole_device_pixels() {
    for scale in SCALES {
        let h = harness(scale);
        h.comp.set_collapsed(true);
        h.advance(0);
        h.advance(400);
        assert!(
            (h.comp.get_panel_left() + 230.0).abs() < 0.5,
            "waits off the edge"
        );
        for reveal in [true, false] {
            h.comp.set_revealed(reveal);
            h.advance(0);
            let _ = h.comp.get_panel_left();
            let mut moving = 0;
            for _ in 0..60 {
                h.advance(5);
                let dev_x = h.comp.get_panel_left() * scale;
                assert!(
                    (dev_x - dev_x.round()).abs() < 0.01,
                    "scale {scale}: the panel is at device x {dev_x}"
                );
                let left = h.comp.get_panel_left();
                if left > -229.0 && left < -1.0 {
                    moving += 1;
                }
            }
            assert!(
                moving > 3,
                "scale {scale}: the slide was seen ({moving} frames)"
            );
        }
    }
}

#[test]
fn hidden_rows_take_no_space() {
    let h = harness(1.0);
    let mut r = rows(1);
    r[2].kept = false;
    r[3].kept = false;
    h.comp.set_rows(ModelRc::new(VecModel::from(r)));
    h.advance(0);
    let f = h.render();
    // Row 4 (index 4) now sits where row 2 did: css y 150.
    assert!(!is_panel(f.px(207, 160)), "row 4 moved up under row 1");
    assert!(is_panel(f.px(207, 262)), "nothing is left where it was");
}
