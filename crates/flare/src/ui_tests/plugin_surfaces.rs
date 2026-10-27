//! The plugins' sidebar sections, the Plugins dropdown, the tabs' hover buttons and the top bar's buttons (plugins
//! phase 4), headless: what each draws, where, what a click reports, and that a plugin's button never lands on the
//! agent's ring or against the window buttons.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::menus_ui::{GLYPH_BADGE_PLUS, glyph_path};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{
    AppWindow, PluginButton, PluginRow, StripGeom, TabRow, TabSidebarPreview, TabStripPreview,
};

/// `Theme.accent-hi`: a ticked checkbox and a ticked button wear it.
const ACCENT: [u8; 4] = [246, 162, 91, 255];

fn header(sid: &str, name: &str, title: &str, folded: bool) -> PluginRow {
    PluginRow {
        kind: 0,
        sid: sid.into(),
        text: name.into(),
        sub: title.into(),
        check: -1,
        folded,
        ..PluginRow::default()
    }
}

fn item(sid: &str, id: &str, text: &str, sub: &str, check: i32) -> PluginRow {
    PluginRow {
        kind: 1,
        sid: sid.into(),
        id: id.into(),
        text: text.into(),
        sub: sub.into(),
        check,
        ..PluginRow::default()
    }
}

/// A section with a ticked item, an unticked one with a second line, and a plain one.
fn todo_rows() -> Vec<PluginRow> {
    vec![
        header("todo/todos", "Session to-dos", "To-dos", false),
        item("todo/todos", "a", "Check the build", "", 1),
        item("todo/todos", "b", "Read the log", "api", 0),
        item("todo/todos", "c", "Plain", "", -1),
    ]
}

fn side(scale: f32, plugin: Vec<PluginRow>) -> Harness<TabSidebarPreview> {
    let h = Harness::new(scale, 700, 500, move || {
        let c = TabSidebarPreview::new().expect("the preview builds");
        let rows: Vec<TabRow> = (0..2)
            .map(|i| TabRow {
                id: format!("t{i}").into(),
                label: format!("tab {i}").into(),
                kept: true,
                prog: -1.0,
                ..TabRow::default()
            })
            .collect();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_plugin_rows(ModelRc::new(VecModel::from(plugin)));
        c
    });
    h.advance(0);
    h
}

#[test]
fn a_section_takes_the_room_its_rows_need_under_the_tab_list_and_none_without_rows() {
    for s in SCALES {
        let h = side(s, todo_rows());
        // 4 padding + a 28 px header + two 28 px rows + one 40 px row with a second line + 6 padding, under a hairline.
        let want = 4.0 + 28.0 + 28.0 + 40.0 + 28.0 + 6.0;
        let got = h.comp.get_sections_h();
        assert!((got - want).abs() <= 1.5, "scale {s}: {got} for {want}");
        // It sits right above the foot (the New tab button, 51 px).
        let y = h.comp.get_sections_y();
        assert!(
            (y + got - (500.0 - 51.0)).abs() <= 1.5,
            "scale {s}: ends at {}",
            y + got
        );
        // None: no room.
        let bare = side(s, Vec::new());
        assert_eq!(bare.comp.get_sections_h(), 0.0);
        // Folded: the header alone.
        let h = side(s, vec![header("todo/todos", "Session to-dos", "", true)]);
        assert!(
            (h.comp.get_sections_h() - (4.0 + 28.0 + 6.0)).abs() <= 1.5,
            "scale {s}"
        );
    }
}

#[test]
fn a_ticked_item_draws_an_accent_box_and_the_others_none() {
    let h = side(1.0, todo_rows());
    let f = h.render();
    let y0 = h.comp.get_sections_y() as u32;
    // Rows: the header 4..32, a 32..60, b 60..100, c 100..128 below the section's top.
    let accent = |y_from: u32, y_to: u32| {
        f.count_where(10, y0 + y_from, 40, y0 + y_to, |p| {
            Frame::near(p, ACCENT, 20)
        })
    };
    assert!(accent(32, 60) > 100, "the ticked box is filled");
    assert_eq!(accent(60, 100), 0, "an unticked box is only outlined");
    assert_eq!(accent(100, 128), 0, "a plain item has no box");
    assert_eq!(accent(4, 32), 0, "the header is not a box");
}

#[test]
fn a_header_click_folds_a_right_click_asks_its_menu_and_an_item_tells_box_from_row() {
    let h = side(1.0, todo_rows());
    let folds: Rc<RefCell<Vec<String>>> = Rc::default();
    let menus: Rc<RefCell<Vec<String>>> = Rc::default();
    let items: Rc<RefCell<Vec<(String, String, bool)>>> = Rc::default();
    {
        let f = folds.clone();
        h.comp
            .on_plugin_fold(move |sid| f.borrow_mut().push(sid.to_string()));
        let m = menus.clone();
        h.comp
            .on_plugin_header_menu(move |sid, _, _| m.borrow_mut().push(sid.to_string()));
        let i = items.clone();
        h.comp.on_plugin_item(move |sid, id, on_box| {
            i.borrow_mut()
                .push((sid.to_string(), id.to_string(), on_box))
        });
    }
    let y0 = h.comp.get_sections_y();
    h.click(80.0, y0 + 4.0 + 14.0);
    h.right_click(80.0, y0 + 4.0 + 14.0);
    // The box of b is at x 16..32 in its 40 px row; its text from x 40.
    h.click(24.0, y0 + 60.0 + 20.0);
    h.click(120.0, y0 + 60.0 + 20.0);
    h.click(120.0, y0 + 100.0 + 14.0);
    assert_eq!(*folds.borrow(), ["todo/todos"]);
    assert_eq!(*menus.borrow(), ["todo/todos"]);
    assert_eq!(
        *items.borrow(),
        [
            ("todo/todos".to_string(), "b".to_string(), true),
            ("todo/todos".to_string(), "b".to_string(), false),
            ("todo/todos".to_string(), "c".to_string(), false),
        ]
    );
}

#[test]
fn the_sections_never_crowd_out_the_tab_list() {
    // Six sections of fifty items each: the box stops at 45 percent of the sidebar and scrolls inside.
    let mut rows = Vec::new();
    for s in 0..6 {
        let sid = format!("p{s}/k");
        rows.push(header(&sid, "P", "", false));
        for i in 0..50 {
            rows.push(item(&sid, &format!("i{i}"), "x", "", -1));
        }
    }
    let h = side(1.0, rows);
    assert!(h.comp.get_sections_h() <= 500.0 * 0.45 + 1.0);
    assert!(h.comp.get_sections_h() > 200.0);
}

fn window(vertical: bool) -> Harness<AppWindow> {
    let h = Harness::new(1.0, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.comp.set_layout_vertical(vertical);
    h.comp.set_has_tabs(true);
    h.advance(0);
    h
}

fn top(n: usize) -> Vec<PluginButton> {
    (0..n)
        .map(|i| PluginButton {
            cmd: if i == 0 {
                glyph_path(GLYPH_BADGE_PLUS).into()
            } else {
                "".into()
            },
            letter: format!("{}", (b'A' + i as u8) as char).into(),
            tip: format!("Plugin {i}: do").into(),
            checked: i == 1,
            enabled: true,
        })
        .collect()
}

#[test]
fn the_plugins_corner_keeps_its_gap_from_the_window_buttons_in_both_bars() {
    for vertical in [false, true] {
        let h = window(vertical);
        assert_eq!(
            h.comp.get_plugin_bar_w(),
            0.0,
            "nothing to draw takes no room"
        );
        h.comp
            .set_plugin_top_buttons(ModelRc::new(VecModel::from(top(3))));
        h.advance(0);
        // Three 26 px chips 4 apart and the 10 px gap.
        let w = h.comp.get_plugin_bar_w();
        assert!(
            (w - (3.0 * 26.0 + 2.0 * 4.0 + 10.0)).abs() <= 1.0,
            "{vertical}: {w}"
        );
        let end = h.comp.get_plugin_bar_x() + w;
        let buttons = h.comp.get_title_buttons_x();
        assert!(
            end <= buttons + 0.5,
            "{vertical}: the corner ends at {end}, the buttons start at {buttons}"
        );
        // What is drawn ends at least 18 px before the first window button's box (the gap and the buttons' padding).
        let bare = window(vertical).render();
        let f = h.render();
        let mut right = 0;
        for y in 0..36 {
            for x in 0..1000 {
                if !Frame::near(bare.px(x, y), f.px(x, y), 3) {
                    right = right.max(x);
                }
            }
        }
        assert!(right > 0, "{vertical}: the chips are drawn");
        assert!(
            (right as f32) < buttons + 8.0 - 18.0 + 1.0,
            "{vertical}: ink at {right}, buttons at {buttons}"
        );
    }
}

#[test]
fn the_plugins_button_shows_in_the_horizontal_bar_only_and_opens_the_dropdown_under_it() {
    let h = window(false);
    let clicked = Rc::new(RefCell::new(0));
    {
        let c = clicked.clone();
        h.comp.on_plugins_clicked(move || *c.borrow_mut() += 1);
    }
    h.comp
        .set_plugin_rows(ModelRc::new(VecModel::from(todo_rows())));
    h.advance(0);
    let (x, w) = (h.comp.get_plugin_bar_x(), h.comp.get_plugin_bar_w());
    assert!(w > 30.0, "the Plugins button takes room: {w}");
    h.click(x + 10.0, 18.0);
    assert_eq!(*clicked.borrow(), 1);
    // Open: the card hangs under the bar, right-aligned to the button, with the sections in it.
    h.comp.set_plugins_open(true);
    h.advance(0);
    assert!(h.comp.get_overlay_open());
    let (cx, cy, cw, ch) = (
        h.comp.get_plugins_card_x(),
        h.comp.get_plugins_card_y(),
        h.comp.get_plugins_card_w(),
        h.comp.get_plugins_card_h(),
    );
    assert!(cy >= 36.0, "under the bar: {cy}");
    assert!(
        (cx + cw - (x + w - 10.0)).abs() <= 1.0,
        "right edge {} for {}",
        cx + cw,
        x + w - 10.0
    );
    assert!(ch > 100.0 && cx >= 8.0, "{cx} {cw} {ch}");
    // A pick in it: the item, with its box.
    let items: Rc<RefCell<Vec<(String, bool)>>> = Rc::default();
    {
        let i = items.clone();
        h.comp
            .on_plugin_item(move |_, id, on_box| i.borrow_mut().push((id.to_string(), on_box)));
    }
    h.click(cx + 24.0, cy + 1.0 + 60.0 + 20.0);
    assert_eq!(*items.borrow(), [("b".to_string(), true)]);
    // A press outside closes it.
    let dismissed = Rc::new(RefCell::new(0));
    {
        let d = dismissed.clone();
        h.comp.on_plugins_dismissed(move || *d.borrow_mut() += 1);
    }
    h.click(100.0, 400.0);
    assert_eq!(*dismissed.borrow(), 1);
    // In the vertical layout the sections are in the sidebar: no Plugins button.
    let v = window(true);
    v.comp
        .set_plugin_rows(ModelRc::new(VecModel::from(todo_rows())));
    v.advance(0);
    assert_eq!(v.comp.get_plugin_bar_w(), 0.0);
}

/// Three named tabs of 150 css px with a ring on tab 1; its hover buttons as given.
fn strip_rows(hov: i32) -> Vec<TabRow> {
    (0..3)
        .map(|i| {
            let mut r = TabRow {
                id: format!("t{i}").into(),
                label: "".into(),
                tip: format!("C:\\t{i}").into(),
                kept: true,
                mode: 0,
                x: i * 150,
                w: 150,
                prog: -1.0,
                ..TabRow::default()
            };
            if i == 1 {
                r.mark_kind = 1;
                r.mark_state = 1;
                r.hov_count = hov;
                r.hov1_cmd = glyph_path(GLYPH_BADGE_PLUS).into();
                r.hov1_tip = "Plugin p: add".into();
                r.hov2_letter = "Q".into();
                r.hov2_on = true;
            }
            r
        })
        .collect()
}

fn strip(rows: Vec<TabRow>) -> Harness<TabStripPreview> {
    let h = Harness::new(1.0, 600, 36, move || {
        let c = TabStripPreview::new().unwrap();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_geom(StripGeom {
            plus_x: 460,
            plus_w: 36,
            ..StripGeom::default()
        });
        c
    });
    h.advance(0);
    h
}

/// Where two frames differ: the box of every pixel that is not the same.
fn diff(a: &Frame, b: &Frame) -> Option<(u32, u32, u32, u32)> {
    let mut out: Option<(u32, u32, u32, u32)> = None;
    for y in 0..a.h {
        for x in 0..a.w {
            if !Frame::near(a.px(x, y), b.px(x, y), 3) {
                out = Some(match out {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    out
}

#[test]
fn hover_buttons_show_on_a_hovered_tab_left_of_its_end_and_never_over_the_ring() {
    let hovered = |hov: i32| {
        let h = strip(strip_rows(hov));
        h.move_to(150.0 + 40.0, 18.0);
        h.advance(400);
        h.render()
    };
    let (none, two, one) = (hovered(0), hovered(2), hovered(1));
    let (x0, y0, x1, y1) = diff(&none, &two).expect("the buttons are drawn");
    // Two 22 px chips 3 apart: 47 px, 20 px tall on the tab's middle.
    assert!((x1 + 1 - x0).abs_diff(47) <= 2, "{x0}..{x1}");
    assert!(y0 >= 7 && y1 <= 28, "{y0}..{y1}");
    // The end box (the ring, and the X that slides in on hover: 34 px) starts 10 + 34 px from the tab's right
    // edge; the buttons end 4 px before it.
    let end_left = 150 + 150 - 10 - 34;
    assert!(
        x1 < end_left - 2,
        "a button reaches {x1}, the end box starts at {end_left}"
    );
    // One: one chip.
    let (a0, _, a1, _) = diff(&none, &one).expect("one button");
    assert!((a1 + 1 - a0).abs_diff(22) <= 2, "{a0}..{a1}");
    // The ticked one wears the accent.
    assert!(two.count_where(x0, y0, x1 + 1, y1 + 1, |p| Frame::near(p, ACCENT, 30)) > 10);
    // Not hovered: nothing.
    let rest = |hov: i32| strip(strip_rows(hov)).render();
    assert_eq!(diff(&rest(0), &rest(2)), None);
}

#[test]
fn hover_buttons_are_never_drawn_on_a_narrow_or_an_icon_tab_and_a_click_names_the_tab_and_place() {
    let mut rows = strip_rows(2);
    rows[1].w = 100;
    rows[2].x = 250;
    let narrow = |rows: Vec<TabRow>| {
        let h = strip(rows);
        h.move_to(150.0 + 30.0, 18.0);
        h.advance(400);
        h.render()
    };
    let mut bare = rows.clone();
    bare[1].hov_count = 0;
    assert_eq!(
        diff(&narrow(bare), &narrow(rows)),
        None,
        "under 110 px: none"
    );
    let mut icon = strip_rows(2);
    icon[1].mode = 1;
    icon[1].w = 40;
    icon[2].x = 190;
    let mut icon_bare = icon.clone();
    icon_bare[1].hov_count = 0;
    assert_eq!(
        diff(&narrow(icon_bare), &narrow(icon)),
        None,
        "an icon tab: none"
    );

    let h = strip(strip_rows(2));
    let got: Rc<RefCell<Vec<(String, i32)>>> = Rc::default();
    let pressed = Rc::new(RefCell::new(0));
    {
        let g = got.clone();
        h.comp
            .on_hover_action(move |id, i| g.borrow_mut().push((id.to_string(), i)));
        let p = pressed.clone();
        h.comp.on_press(move |_, _, _, _| *p.borrow_mut() += 1);
    }
    h.move_to(150.0 + 40.0, 18.0);
    h.advance(400);
    // The chips end 4 px before the 34 px end box: the second at 150+102-22 .. 150+102, the first 25 px before.
    h.click(150.0 + 102.0 - 11.0, 18.0);
    h.click(150.0 + 102.0 - 25.0 - 11.0, 18.0);
    assert_eq!(
        *got.borrow(),
        [("t1".to_string(), 1), ("t1".to_string(), 0)]
    );
    assert_eq!(
        *pressed.borrow(),
        0,
        "a click on a button never starts a carry"
    );
}

#[test]
fn a_sidebar_row_draws_its_hover_buttons_left_of_the_ring() {
    let make = |hov: i32| {
        let h = Harness::new(1.0, 700, 500, move || {
            let c = TabSidebarPreview::new().expect("the preview builds");
            let rows: Vec<TabRow> = strip_rows(hov)
                .into_iter()
                .map(|mut r| {
                    r.label = "api".into();
                    r
                })
                .collect();
            c.set_rows(ModelRc::new(VecModel::from(rows)));
            c
        });
        h.advance(0);
        // Row 1 is the second 46 px card under the 50 px find box (4 px apart).
        h.move_to(100.0, 50.0 + 46.0 + 4.0 + 23.0);
        h.advance(400);
        h.render()
    };
    let (x0, _, x1, _) = diff(&make(0), &make(2)).expect("the buttons are drawn");
    assert!((x1 + 1 - x0).abs_diff(47) <= 2, "{x0}..{x1}");
    // The row's end box (ring and X) starts 8 + 38 px from the card's right edge (the card is 8 px in from 220).
    let end_left = 220 - 8 - 8 - 38;
    assert!(
        x1 < end_left,
        "a button reaches {x1}, the end starts at {end_left}"
    );
}
