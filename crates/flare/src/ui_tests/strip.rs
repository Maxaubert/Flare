//! component and flow tests for the tab strip (Tasks 20 and 31).

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use flare_tabs::strip_layout::{layout_strip, room_for_plan};
use flare_tabs::strip_plan::{StripSlot, plan_strip};
use slint::{Color, Model, ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{StripGeom, TabRow, TabStripPreview};

const GROUND: [u8; 4] = [23, 23, 23, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];

fn is_divider(p: [u8; 4]) -> bool {
    Frame::near(p, DIVIDER, 2)
}

fn is_ground(p: [u8; 4]) -> bool {
    Frame::near(p, GROUND, 2)
}

fn bright(p: [u8; 4]) -> bool {
    (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 90
}

/// Rows and geometry for `n` tabs named `t0..`, the `active`th in front, in a strip
/// `strip_css` wide at `scale`.
fn rows_for(n: usize, active: usize, scale: f32, strip_css: f64) -> (Vec<TabRow>, StripGeom) {
    let ids: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
    let plan = plan_strip(
        &ids,
        Some(ids[active].as_str()),
        &HashSet::new(),
        room_for_plan(strip_css),
    );
    let phys = (strip_css * f64::from(scale)).round() as i32;
    let lay = layout_strip(&plan, phys, f64::from(scale));
    let hidden = plan.iter().filter(|s| **s == StripSlot::Hidden).count();
    let rows = (0..n)
        .map(|i| {
            let slot = lay.slots[i];
            TabRow {
                id: ids[i].as_str().into(),
                label: format!("tab {i}").into(),
                active: i == active,
                mode: match plan[i] {
                    StripSlot::Full => 0,
                    StripSlot::Icon => 1,
                    StripSlot::Hidden => 2,
                },
                x: slot.map_or(0, |s| s.x),
                w: slot.map_or(0, |s| s.w),
                ..Default::default()
            }
        })
        .collect();
    let geom = StripGeom {
        more_x: lay.more.map_or(0, |m| m.x),
        more_w: lay.more.map_or(0, |m| m.w),
        plus_x: lay.plus.x,
        plus_w: lay.plus.w,
        more_count: hidden as i32,
        dots: ModelRc::default(),
    };
    (rows, geom)
}

struct Rig {
    h: Harness<TabStripPreview>,
    model: Rc<VecModel<TabRow>>,
}

fn rig(scale: f32, css_w: u32, strip_x: f32, rows: Vec<TabRow>, geom: StripGeom) -> Rig {
    let model = Rc::new(VecModel::from(rows));
    let m2 = model.clone();
    let h = Harness::new(scale, css_w, 36, move || {
        let c = TabStripPreview::new().unwrap();
        c.set_rows(ModelRc::from(m2));
        c.set_geom(geom);
        c.set_strip_x(strip_x);
        c.set_new_tab_title("New tab".into());
        c
    });
    Rig { h, model }
}

#[test]
fn every_tab_edge_is_a_whole_device_pixel_with_a_crisp_divider() {
    for s in SCALES {
        let (rows, geom) = rows_for(3, 1, s, 1000.0);
        let slots: Vec<(i32, i32)> = rows.iter().map(|r| (r.x, r.w)).collect();
        let r = rig(s, 1000, 0.0, rows, geom);
        let f = r.h.render();
        let y = r.h.phys(17.0);
        let hair = s.floor().max(1.0) as i32;
        for (x, w) in slots {
            let end = (x + w) as u32;
            for k in 1..=hair as u32 {
                assert!(
                    is_divider(f.px(end - k, y)),
                    "scale {s}: column {} of the divider at {end}",
                    end - k
                );
            }
            assert!(
                !is_divider(f.px(end - hair as u32 - 1, y)),
                "scale {s}: the divider at {end} is wider than {hair}"
            );
            assert!(
                is_ground(f.px(end, y)),
                "scale {s}: the column after the divider at {end} is {:?}",
                f.px(end, y)
            );
        }
    }
}

#[test]
fn the_active_tab_has_a_snapped_two_pixel_rule() {
    for (s, want) in [(1.0f32, 2u32), (1.5, 3), (2.25, 5)] {
        let (rows, geom) = rows_for(3, 1, s, 1000.0);
        let cx = (rows[1].x + rows[1].w / 2) as u32;
        let r = rig(s, 1000, 0.0, rows, geom);
        let f = r.h.render();
        let n = f.run_len_v(cx, 0, |p| Frame::near(p, ACCENT_HI, 3));
        assert_eq!(n, want, "scale {s}");
    }
}

#[test]
fn names_are_centred_and_elided() {
    let (mut rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    rows[0].label = "api".into();
    rows[2].label = "abcdefghij".repeat(6).into();
    let r = rig(1.0, 1000, 0.0, rows, geom);
    let f = r.h.render();
    // Tab 0 is 0..150, so its name box is 34..118.
    let (x0, _, x1, _) = f
        .bbox_where(34, 0, 118, 35, bright)
        .expect("the name has ink");
    let centre = (x0 + x1) as f32 / 2.0;
    assert!((centre - 76.0).abs() <= 2.0, "ink centre {centre}");
    // Tab 2 is 300..450: name box 334..418, end box 424..440.
    let (_, _, right, _) = f
        .bbox_where(334, 0, 450, 35, bright)
        .expect("the elided name has ink");
    assert!((418 - 8..418).contains(&right), "ink ends at {right}");
    assert_eq!(
        f.count_where(418, 0, 449, 35, bright),
        0,
        "ink crossed into the end box"
    );
}

type PressLog = Rc<RefCell<Vec<(String, i32, f32, f32)>>>;

#[test]
fn a_press_reports_the_window_x_and_the_strips_origin() {
    let (rows, geom) = rows_for(3, 0, 1.0, 950.0);
    let r = rig(1.0, 1000, 50.0, rows, geom);
    let presses: PressLog = Rc::default();
    let moves: Rc<RefCell<Vec<f32>>> = Rc::default();
    let releases = Rc::new(RefCell::new(0));
    {
        let p = presses.clone();
        r.h.comp
            .on_press(move |id, i, x, sx| p.borrow_mut().push((id.to_string(), i, x, sx)));
        let m = moves.clone();
        r.h.comp.on_drag_moved(move |x| m.borrow_mut().push(x));
        let rel = releases.clone();
        r.h.comp.on_drag_released(move || *rel.borrow_mut() += 1);
    }
    r.h.move_to(210.0, 18.0);
    r.h.press(210.0, 18.0, slint::platform::PointerEventButton::Left);
    {
        let p = presses.borrow();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "t1");
        assert_eq!(p[0].1, 1);
        assert!((p[0].2 - 210.0).abs() < 0.5, "x {}", p[0].2);
        assert!((p[0].3 - 50.0).abs() < 0.5, "origin {}", p[0].3);
    }
    r.h.move_to(240.0, 18.0);
    assert!((moves.borrow().last().copied().unwrap() - 240.0).abs() < 0.5);
    r.h.release(240.0, 18.0, slint::platform::PointerEventButton::Left);
    assert_eq!(*releases.borrow(), 1);
}

#[test]
fn middle_click_closes_and_right_click_opens_the_tab_menu() {
    let (rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    let r = rig(1.0, 1000, 0.0, rows, geom);
    let closed: Rc<RefCell<Vec<String>>> = Rc::default();
    let menus: Rc<RefCell<Vec<(String, f32, f32)>>> = Rc::default();
    let pressed = Rc::new(RefCell::new(0));
    {
        let c = closed.clone();
        r.h.comp
            .on_closed(move |id| c.borrow_mut().push(id.to_string()));
        let m = menus.clone();
        r.h.comp
            .on_tab_menu(move |id, x, y| m.borrow_mut().push((id.to_string(), x, y)));
        let p = pressed.clone();
        r.h.comp.on_press(move |_, _, _, _| *p.borrow_mut() += 1);
    }
    r.h.middle_click(60.0, 18.0);
    assert_eq!(*closed.borrow(), vec!["t0".to_string()]);
    r.h.right_click(60.0, 18.0);
    let m = menus.borrow();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0, "t0");
    assert!(
        (m[0].1 - 60.0).abs() < 0.5 && (m[0].2 - 18.0).abs() < 0.5,
        "{:?}",
        m[0]
    );
    assert_eq!(*pressed.borrow(), 0);
}

#[test]
fn a_press_on_the_x_closes_and_never_starts_a_carry() {
    let (rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    let r = rig(1.0, 1000, 0.0, rows, geom);
    let closed: Rc<RefCell<Vec<String>>> = Rc::default();
    let pressed = Rc::new(RefCell::new(0));
    {
        let c = closed.clone();
        r.h.comp
            .on_closed(move |id| c.borrow_mut().push(id.to_string()));
        let p = pressed.clone();
        r.h.comp.on_press(move |_, _, _, _| *p.borrow_mut() += 1);
    }
    // Tab 1 is 150..300: its X box is 274..290.
    r.h.click(282.0, 17.0);
    assert_eq!(*closed.borrow(), vec!["t1".to_string()]);
    assert_eq!(*pressed.borrow(), 0);
}

#[test]
fn the_x_is_hidden_until_the_tab_is_active_or_hovered() {
    let (rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    let r = rig(1.0, 1000, 0.0, rows, geom);
    r.h.move_to(900.0, 18.0);
    r.h.advance(200);
    let f = r.h.render();
    // Tab 0: X box 124..140 by 9..26.
    assert_eq!(
        f.count_where(124, 9, 140, 26, |p| !is_ground(p)),
        0,
        "the X shows on a resting background tab"
    );
    r.h.move_to(60.0, 18.0);
    r.h.advance(200);
    let f = r.h.render();
    let ink = f.count_where(124, 9, 140, 26, |p| {
        (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 70
    });
    assert!(ink > 0, "no X glyph after hover");
}

#[test]
fn the_plus_button_clicks_and_right_clicks() {
    let (rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    let (px, pw) = (geom.plus_x as f32, geom.plus_w as f32);
    let r = rig(1.0, 1000, 0.0, rows, geom);
    let news = Rc::new(RefCell::new(0));
    let menus: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    {
        let n = news.clone();
        r.h.comp.on_new_tab(move || *n.borrow_mut() += 1);
        let m = menus.clone();
        r.h.comp
            .on_plus_menu(move |x, y| m.borrow_mut().push((x, y)));
    }
    let (cx, cy) = (px + pw / 2.0, 18.0);
    r.h.click(cx, cy);
    assert_eq!(*news.borrow(), 1);
    r.h.right_click(cx, cy);
    assert_eq!(menus.borrow().len(), 1);
    assert_eq!(*news.borrow(), 1);
    // The hover background fills 28 x (36 - 8 - 1) css px.
    r.h.move_to(cx, cy);
    r.h.advance(50);
    let f = r.h.render();
    let hov = |p: [u8; 4]| Frame::near(p, [48, 48, 48, 255], 4);
    let (x0, y0, x1, y1) = f
        .bbox_where(px as u32 - 4, 0, px as u32 + 36, 36, hov)
        .expect("the hover background");
    assert_eq!((x1 - x0 + 1, y1 - y0 + 1), (28, 27));
}

#[test]
fn icon_tabs_are_forty_wide_and_have_no_x() {
    for s in [1.0f32, 2.25] {
        let (rows, geom) = rows_for(20, 0, s, 600.0);
        let icon = rows
            .iter()
            .position(|r| r.mode == 1)
            .expect("an icon tab in the plan");
        let (ix, iw) = (rows[icon].x, rows[icon].w);
        let r = rig(s, 600, 0.0, rows, geom);
        let pressed: Rc<RefCell<Vec<String>>> = Rc::default();
        let closed = Rc::new(RefCell::new(0));
        {
            let p = pressed.clone();
            r.h.comp
                .on_press(move |id, _, _, _| p.borrow_mut().push(id.to_string()));
            let c = closed.clone();
            r.h.comp.on_closed(move |_| *c.borrow_mut() += 1);
        }
        let cx = (ix as f32 + iw as f32 / 2.0) / s;
        r.h.move_to(cx, 18.0);
        r.h.advance(50);
        let f = r.h.render();
        let hov = |p: [u8; 4]| Frame::near(p, [37, 37, 37, 255], 3);
        let run = f.run_len_h(r.h.phys(4.0), ix as u32, hov);
        let hair = s.floor().max(1.0) as i32;
        let total = run as i32 + hair;
        assert!(
            (total - (40.0 * s).round() as i32).abs() <= 1,
            "scale {s}: icon tab is {total} device px"
        );
        // Where an X would be: a press, not a close.
        let x_where = (ix + iw) as f32 / s - 18.0;
        r.h.click(x_where, 17.0);
        assert_eq!(pressed.borrow().len(), 1, "scale {s}");
        assert_eq!(*closed.borrow(), 0, "scale {s}");
    }
}

#[test]
fn the_more_button_shows_the_count_and_up_to_four_dots() {
    let (rows, _) = rows_for(3, 1, 1.0, 1000.0);
    let colours = [
        Color::from_rgb_u8(255, 0, 0),
        Color::from_rgb_u8(0, 255, 0),
        Color::from_rgb_u8(0, 0, 255),
        Color::from_rgb_u8(255, 255, 0),
        Color::from_rgb_u8(255, 0, 255),
    ];
    let geom = StripGeom {
        more_x: 400,
        more_w: 72,
        plus_x: 476,
        plus_w: 28,
        more_count: 7,
        dots: ModelRc::from(Rc::new(VecModel::from(colours.to_vec()))),
    };
    let r = rig(1.0, 1000, 0.0, rows, geom);
    let f = r.h.render();
    let present: Vec<bool> = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [255, 0, 255],
    ]
    .iter()
    .map(|c| {
        f.count_where(400, 0, 472, 36, |p| {
            Frame::near(p, [c[0], c[1], c[2], 255], 8)
        }) > 0
    })
    .collect();
    assert_eq!(present, vec![true, true, true, true, false]);
    let reds = f.count_where(400, 0, 472, 36, |p| Frame::near(p, [255, 0, 0, 255], 8));
    assert!((14..=40).contains(&reds), "a 6 px dot, got {reds} px");
    let got: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    let g = got.clone();
    r.h.comp
        .on_more_clicked(move |x, y| g.borrow_mut().push((x, y)));
    r.h.click(436.0, 18.0);
    let g = got.borrow();
    assert_eq!(g.len(), 1);
    assert!(
        (g[0].0 - 472.0).abs() < 0.5 && (g[0].1 - 31.0).abs() < 0.5,
        "{:?}",
        g[0]
    );
}

#[test]
fn a_carried_tab_follows_the_pointer_and_neighbours_slide() {
    let (mut rows, geom) = rows_for(3, 2, 1.0, 1000.0);
    let r = {
        rows[1].active = false;
        rig(1.0, 1000, 0.0, rows, geom)
    };
    let f = r.h.render();
    // Tab 1 is 150..300: its divider is the column 299.
    assert!(is_divider(f.px(299, 17)));
    r.h.comp.set_carrying(true);
    let mut one = r.model.row_data(1).unwrap();
    one.carried = true;
    one.offset = 40.0;
    r.model.set_row_data(1, one);
    let mut two = r.model.row_data(2).unwrap();
    two.offset = -150.0;
    r.model.set_row_data(2, two);
    r.h.advance(0);
    let f = r.h.render();
    assert!(
        is_divider(f.px(339, 17)),
        "the carried tab has not moved 40 px"
    );
    assert!(!is_divider(f.px(299, 17)));
    // Tab 2 (active, so its rule marks its left edge) slides 300 -> 150.
    let left = |f: &Frame| {
        (0..1000u32)
            .find(|x| Frame::near(f.px(*x, 0), ACCENT_HI, 3))
            .unwrap()
    };
    assert!(
        left(&f) > 150 && left(&f) <= 300,
        "left edge {} at the start",
        left(&f)
    );
    r.h.advance(85);
    let mid = left(&r.h.render());
    assert!(mid > 150 && mid < 300, "mid slide at {mid}");
    r.h.advance(100);
    assert_eq!(left(&r.h.render()), 150);
}

#[test]
fn the_window_handle_goes_away_while_a_drag_is_in_flight() {
    let (rows, geom) = rows_for(3, 1, 1.0, 1000.0);
    let r = rig(1.0, 1000, 0.0, rows, geom);
    assert!(r.h.comp.get_has_move_area());
    r.h.comp.set_drag_in_flight(true);
    assert!(!r.h.comp.get_has_move_area());
}
