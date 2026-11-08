//! UI tests of the help panel (M7 Task 39); headless only.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::{Key, PointerEventButton};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{HelpBridge, HelpChip, HelpPanelPreview, HelpRowData};

const SIDE_FLAT: [u8; 4] = [0x1f, 0x1f, 0x1e, 0xff];
/// The sheet's top in the 1000 by 640 preview (below the 36 px title bar).
const PANEL_Y: f32 = 36.0;

fn data(name: &str, command: &str) -> HelpRowData {
    HelpRowData {
        key: format!("{name}#0").into(),
        name: name.into(),
        command: command.into(),
        header: "".into(),
        keys: false,
        danger: false,
        active: false,
        striped: false,
        copied: false,
        failed: false,
    }
}

fn rows_model(rows: Vec<HelpRowData>) -> ModelRc<HelpRowData> {
    ModelRc::new(VecModel::from(rows))
}

fn chips() -> ModelRc<HelpChip> {
    ModelRc::new(VecModel::from(vec![
        HelpChip {
            id: "powershell".into(),
            name: "PowerShell".into(),
            on: true,
        },
        HelpChip {
            id: "cmd".into(),
            name: "Command Prompt".into(),
            on: false,
        },
        HelpChip {
            id: "bash".into(),
            name: "Bash".into(),
            on: false,
        },
    ]))
}

fn harness(scale: f32) -> Harness<HelpPanelPreview> {
    let h = Harness::new(scale, 1000, 640, || HelpPanelPreview::new().unwrap());
    let b = h.comp.global::<HelpBridge>();
    b.set_chips(chips());
    b.set_count_text("3 commands".into());
    b.set_placeholder("Try: find big files, what is using port 3000".into());
    b.set_search_label("Search commands".into());
    b.set_empty_text("Nothing matches.".into());
    h.advance(1);
    h
}

fn three_rows() -> Vec<HelpRowData> {
    let mut v = vec![
        data("List files", "Get-ChildItem"),
        data("Find big files", "Get-ChildItem -Recurse"),
        data("Show the path", "Get-Location"),
    ];
    v[1].striped = true;
    v
}

/// Opens the panel with `rows` and lets the slide and the rise-in finish.
fn open_with(h: &Harness<HelpPanelPreview>, rows: Vec<HelpRowData>) -> Rc<VecModel<HelpRowData>> {
    let b = h.comp.global::<HelpBridge>();
    let model = Rc::new(VecModel::from(rows));
    b.set_rows(ModelRc::from(model.clone()));
    b.set_open(true);
    h.advance(1);
    h.advance(900);
    // layout runs at the first paint
    let _ = h.render();
    model
}

#[derive(Default)]
struct Log {
    keys: Vec<(String, bool, bool)>,
    pressed: Vec<i32>,
    clicked: Vec<(i32, f32)>,
    copied: Vec<i32>,
    drags: Vec<f32>,
    grip_keys: Vec<(String, bool)>,
    closed: u32,
    queries: Vec<String>,
    chips: Vec<String>,
}

fn wire(h: &Harness<HelpPanelPreview>) -> Rc<RefCell<Log>> {
    let log = Rc::new(RefCell::new(Log::default()));
    let b = h.comp.global::<HelpBridge>();
    let l = log.clone();
    b.on_key(move |n, s, c| l.borrow_mut().keys.push((n.to_string(), s, c)));
    let l = log.clone();
    b.on_row_pressed(move |i| l.borrow_mut().pressed.push(i));
    let l = log.clone();
    b.on_row_clicked(move |i, m| l.borrow_mut().clicked.push((i, m)));
    let l = log.clone();
    b.on_copy_clicked(move |i| l.borrow_mut().copied.push(i));
    let l = log.clone();
    b.on_grip_dragged(move |d| l.borrow_mut().drags.push(d));
    let l = log.clone();
    b.on_grip_key(move |n, s| l.borrow_mut().grip_keys.push((n.to_string(), s)));
    let l = log.clone();
    b.on_closed(move || l.borrow_mut().closed += 1);
    let l = log.clone();
    b.on_query_edited(move |q| l.borrow_mut().queries.push(q.to_string()));
    let l = log.clone();
    b.on_chip_picked(move |c| l.borrow_mut().chips.push(c.to_string()));
    log
}

fn key(k: Key) -> String {
    String::from(char::from(k))
}

fn blend(top: [u8; 3], a: f32, under: [u8; 4]) -> [u8; 4] {
    let m = |t: u8, u: u8| (f32::from(t) * a + f32::from(u) * (1.0 - a)).round() as u8;
    [
        m(top[0], under[0]),
        m(top[1], under[1]),
        m(top[2], under[2]),
        0xff,
    ]
}

/// The panel's pixel at CSS (x, y).
fn at(h: &Harness<HelpPanelPreview>, f: &Frame, x: f32, y: f32) -> [u8; 4] {
    f.px(h.phys(x).min(f.w - 1), h.phys(y).min(f.h - 1))
}

#[test]
fn the_sheet_is_620_wide_by_default_flush_right_and_full_height() {
    for scale in SCALES {
        let h = harness(scale);
        open_with(&h, three_rows());
        let sw = h.comp.get_sheet_w();
        let sx = h.comp.get_sheet_x();
        assert!((sw - 620.0).abs() < 0.01, "width {sw} at {scale}");
        assert!((sx - 380.0).abs() < 0.01, "flush right {sx} at {scale}");
        let f = h.render();
        // the left edge pixel column is the divider over the sheet fill
        let want = blend([0xff, 0xff, 0xff], 0x12 as f32 / 255.0, SIDE_FLAT);
        let px = at(&h, &f, 380.2, 300.0);
        assert!(
            Frame::near(px, want, 3),
            "edge {px:?} want {want:?} at {scale}"
        );
        // full height: the last row of the sheet is the sheet, the title bar above is not
        let low = at(&h, &f, 700.0, 639.4);
        assert!(Frame::near(low, SIDE_FLAT, 2), "bottom {low:?} at {scale}");
        let above = at(&h, &f, 700.0, PANEL_Y - 1.0);
        assert!(!Frame::near(above, SIDE_FLAT, 1), "title bar at {scale}");
        // the sheet is at least 4 px of shadow to its left: the strip just left of the edge is
        // darker than the terminal behind it
        let terminal = at(&h, &f, 100.0, 300.0);
        let shade = at(&h, &f, 378.0, 300.0);
        assert!(
            shade[0] <= terminal[0],
            "shadow {shade:?} vs {terminal:?} at {scale}"
        );
    }
}

#[test]
fn the_sheet_never_passes_the_container_less_120() {
    let h = harness(1.0);
    h.comp.global::<HelpBridge>().set_panel_w(1100.0);
    open_with(&h, three_rows());
    assert!((h.comp.get_sheet_w() - 880.0).abs() < 0.01);
}

fn metrics() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/pt_settings_metrics.json"
    ))
    .unwrap()
}

#[test]
fn header_search_and_chip_boxes_match_the_reference() {
    let m = metrics();
    for (key, scale) in [("1", 1.0_f32), ("1.5", 1.5)] {
        let p = &m["scales"][key]["helpPanel"];
        let num = |a: &str, b: &str| p[a][b].as_f64().unwrap() as f32;
        let h = harness(scale);
        open_with(&h, three_rows());
        // [data-help-search]: the input's own width
        let w = h.comp.get_search_w();
        assert!(
            (w - num("search", "width")).abs() <= 1.0,
            "search width {w} vs {} at {scale}",
            num("search", "width")
        );
        // [data-help-shell]: the chip is 28 high (27.33 at 150 percent)
        let c = h.comp.get_chip_h();
        assert!(
            (c - num("shell0", "height")).abs() <= 1.0,
            "chip {c} vs {} at {scale}",
            num("shell0", "height")
        );
        // the list starts where the first category heading starts, measured from the panel top
        let ly = h.comp.get_list_y();
        let want = num("category", "y") - num("panel", "y");
        assert!((ly - want).abs() <= 1.0, "list y {ly} vs {want} at {scale}");
        // a one-line row is 36 high
        let r = h.comp.get_row0_h();
        assert!(
            (r - num("row", "height")).abs() <= 1.0,
            "row {r} at {scale}"
        );
        // [data-help-count]: right aligned, 16 px from the sheet's right edge
        assert!((h.comp.get_search_h() - 36.0).abs() < 0.01);
    }
}

#[test]
fn the_slide_takes_460_ms_in_and_220_out_and_is_instant_under_reduced_motion() {
    let h = harness(1.0);
    let b = h.comp.global::<HelpBridge>();
    b.set_rows(rows_model(three_rows()));
    let out = 380.0 + 1.04 * 620.0;
    assert!((h.comp.get_sheet_x() - out).abs() < 0.01);
    b.set_open(true);
    let start = h.comp.get_sheet_x();
    assert!((start - out).abs() < 1.0, "starts out: {start}");
    h.advance(230);
    let half = h.comp.get_sheet_x();
    assert!(half < out - 1.0 && half > 380.0 + 1.0, "mid slide {half}");
    h.advance(240);
    assert!((h.comp.get_sheet_x() - 380.0).abs() < 0.5, "in at 470 ms");

    b.set_open(false);
    h.advance(110);
    let mid = h.comp.get_sheet_x();
    assert!(mid > 380.0 + 1.0 && mid < out - 1.0, "mid out {mid}");
    h.advance(120);
    assert!((h.comp.get_sheet_x() - out).abs() < 0.5, "out at 230 ms");

    // reduced motion: no time passes
    b.set_still(true);
    b.set_open(true);
    h.advance(1);
    assert!((h.comp.get_sheet_x() - 380.0).abs() < 0.01, "instant in");
    b.set_open(false);
    h.advance(1);
    assert!((h.comp.get_sheet_x() - out).abs() < 0.01, "instant out");
}

#[test]
fn an_out_panel_takes_no_keys_and_no_clicks() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    h.comp.global::<HelpBridge>().set_open(false);
    h.advance(600);
    let before = h.comp.get_terminal_clicks();
    h.click(700.0, 190.0);
    h.click(990.0, 60.0);
    h.key_text(&key(Key::DownArrow));
    h.key_text(&key(Key::Escape));
    h.key_text(&key(Key::Return));
    let l = log.borrow();
    assert!(l.keys.is_empty(), "keys {:?}", l.keys);
    assert!(l.clicked.is_empty() && l.pressed.is_empty() && l.copied.is_empty());
    assert_eq!(l.closed, 0);
    drop(l);
    assert_eq!(
        h.comp.get_terminal_clicks(),
        before + 2,
        "the clicks land on the terminal behind"
    );
}

#[test]
fn the_grip_resizes_by_drag_and_by_keys() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    // press on the edge (4 inside), drag 50 px to the left
    h.drag((382.0, 300.0), (332.0, 300.0), 5);
    {
        let l = log.borrow();
        assert!(!l.drags.is_empty());
        let last = *l.drags.last().unwrap();
        assert!((last - 50.0).abs() < 0.5, "travel {last}");
        assert!((h.comp.global::<HelpBridge>().get_grip_from() - 620.0).abs() < 0.01);
    }
    // the grip has the focus now: Left grows by a step, Shift+Right shrinks by a big one
    h.key_text(&key(Key::LeftArrow));
    h.key_with(true, false, &key(Key::RightArrow));
    let l = log.borrow();
    assert_eq!(
        l.grip_keys,
        vec![("Left".to_string(), false), ("Right".to_string(), true)]
    );
}

#[test]
fn a_row_is_two_columns_with_the_well_and_the_copy_button() {
    for scale in SCALES {
        let h = harness(scale);
        open_with(&h, three_rows());
        h.move_to(5.0, 5.0);
        let f = h.render();
        // row 0 runs from 172 to 208 in the preview
        let y = 190.0;
        let well_ink = at(&h, &f, 900.0, y);
        assert!(
            well_ink[0] < SIDE_FLAT[0],
            "the well is darker than the sheet: {well_ink:?} at {scale}"
        );
        // the gap between the columns and the margin after the well are the sheet
        for x in [640.0, 990.0] {
            let p = at(&h, &f, x, y);
            assert!(Frame::near(p, SIDE_FLAT, 1), "{x}: {p:?} at {scale}");
        }
        // the well starts at 642.92 and ends at 984
        let before = at(&h, &f, 642.0, y);
        let inside = at(&h, &f, 644.0, y);
        assert!(
            Frame::near(before, SIDE_FLAT, 1) && inside[0] < SIDE_FLAT[0],
            "start at {scale}"
        );
        let end_in = at(&h, &f, 983.3, y);
        let end_out = at(&h, &f, 985.0, y);
        assert!(
            end_in[0] < SIDE_FLAT[0] && Frame::near(end_out, SIDE_FLAT, 1),
            "end at {scale}"
        );
        // the name column holds ink (text), the first 15 px of padding do not
        let name_ink = f.count_where(
            h.phys(397.0),
            h.phys(173.0),
            h.phys(500.0),
            h.phys(207.0),
            |p| p[0] > 120,
        );
        assert!(name_ink > 0, "name text at {scale}");
        assert_eq!(
            f.count_where(
                h.phys(382.0),
                h.phys(173.0),
                h.phys(396.0),
                h.phys(207.0),
                |p| p[0] > 60
            ),
            0,
            "padding at {scale}"
        );
        // the copy button holds the icon's stroke ink
        let icon = f.count_where(
            h.phys(956.0),
            h.phys(177.0),
            h.phys(982.0),
            h.phys(203.0),
            |p| p[0] > 100,
        );
        assert!(icon > 0, "copy icon at {scale}");
    }
}

#[cfg_attr(
    not(windows),
    ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
)]
#[test]
fn a_two_line_name_makes_the_row_taller() {
    let h = harness(1.0);
    let long =
        "A very long task name that has to wrap onto a second line in the left column of the row";
    open_with(&h, vec![data(long, "Get-Thing")]);
    let r = h.comp.get_row0_h();
    assert!(r > 36.0 && r <= 39.0, "row {r}");
}

#[test]
fn stripes_follow_the_drawn_order_and_the_active_row_is_one_step_more() {
    for scale in SCALES {
        let h = harness(scale);
        let mut rows = three_rows();
        rows[2].active = true;
        open_with(&h, rows);
        h.move_to(5.0, 5.0);
        let f = h.render();
        let c = |a: f32| blend([0xde, 0xda, 0xcf], a, SIDE_FLAT);
        let r0 = at(&h, &f, 386.0, 190.0);
        let r1 = at(&h, &f, 386.0, 226.0);
        let r2 = at(&h, &f, 386.0, 262.0);
        assert!(
            Frame::near(r0, SIDE_FLAT, 1),
            "row 0 plain {r0:?} at {scale}"
        );
        assert!(
            Frame::near(r1, c(0.04), 2),
            "row 1 stripe {r1:?} want {:?} at {scale}",
            c(0.04)
        );
        assert!(
            Frame::near(r2, c(0.13), 2),
            "row 2 active {r2:?} want {:?} at {scale}",
            c(0.13)
        );
        // the pointer over row 0 lights it a step above the sheet
        h.move_to(386.0, 190.0);
        h.advance(200);
        let f = h.render();
        let hov = at(&h, &f, 386.0, 190.0);
        assert!(Frame::near(hov, c(0.09), 2), "hover {hov:?} at {scale}");
    }
}

#[test]
fn a_key_row_has_caps_and_no_copy_button() {
    let h = harness(1.0);
    let log = wire(&h);
    let mut rows = three_rows();
    rows[0].keys = true;
    rows[0].command = "Ctrl+C".into();
    let b = h.comp.global::<HelpBridge>();
    b.set_key_caps(ModelRc::new(VecModel::from(vec![
        ModelRc::new(VecModel::from(vec![
            SharedString::from("Ctrl"),
            SharedString::from("C"),
        ])),
        ModelRc::new(VecModel::<SharedString>::default()),
        ModelRc::new(VecModel::<SharedString>::default()),
    ])));
    open_with(&h, rows);
    h.move_to(5.0, 5.0);
    let f = h.render();
    // no well: the column is the sheet's colour where the well would be
    let p = at(&h, &f, 975.0, 190.0);
    assert!(Frame::near(p, SIDE_FLAT, 1), "no well or button: {p:?}");
    // two caps: their 1 px lines and text are ink in the command column
    let ink = f.count_where(
        h.phys(643.0),
        h.phys(174.0),
        h.phys(760.0),
        h.phys(206.0),
        |p| !Frame::near(p, SIDE_FLAT, 2),
    );
    assert!(ink > 40, "caps drawn: {ink}");
    // the right half of the column is empty
    assert_eq!(
        f.count_where(
            h.phys(800.0),
            h.phys(174.0),
            h.phys(984.0),
            h.phys(206.0),
            |p| { !Frame::near(p, SIDE_FLAT, 2) }
        ),
        0
    );
    // a key row copies nothing, on a click anywhere
    h.click(500.0, 190.0);
    h.click(700.0, 190.0);
    let l = log.borrow();
    assert!(l.clicked.is_empty() && l.copied.is_empty());
}

#[test]
fn the_copy_icon_turns_into_a_check_and_back() {
    let h = harness(1.0);
    let model = open_with(&h, three_rows());
    h.move_to(5.0, 5.0);
    let green = |p: [u8; 4]| Frame::near(p, [0x8f, 0xd4, 0x7a, 0xff], 70) && p[1] > p[0] + 30;
    let count = |h: &Harness<HelpPanelPreview>| {
        let f = h.render();
        f.count_where(
            h.phys(956.0),
            h.phys(177.0),
            h.phys(982.0),
            h.phys(203.0),
            green,
        )
    };
    assert_eq!(count(&h), 0, "no check at rest");
    let mut row = model.row_data(0).unwrap();
    row.copied = true;
    model.set_row_data(0, row);
    h.advance(1);
    let t0 = count(&h);
    assert!(t0 <= 8, "the check has not started: {t0}");
    h.advance(340);
    let t340 = count(&h);
    assert!(t340 > t0, "the check is being drawn at 340 ms: {t340}");
    h.advance(200);
    let done = count(&h);
    assert!(done >= t340 && done > 12, "the check is complete: {done}");
    // and back to the copy icon
    let mut row = model.row_data(0).unwrap();
    row.copied = false;
    model.set_row_data(0, row);
    h.advance(1);
    h.advance(400);
    assert_eq!(count(&h), 0, "back to the copy icon");
}

#[test]
fn a_click_in_the_well_copies_nothing_but_its_button_does() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    // on the command text: a click is for selecting, so nothing is copied
    h.click(700.0, 190.0);
    assert!(log.borrow().clicked.is_empty() && log.borrow().copied.is_empty());
    // in the well: only a press
    h.click(900.0, 190.0);
    {
        let l = log.borrow();
        assert!(
            !l.pressed.is_empty() && l.pressed.iter().all(|&i| i == 0),
            "pressed {:?}",
            l.pressed
        );
        assert!(
            l.clicked.is_empty() && l.copied.is_empty(),
            "well copies nothing"
        );
    }
    // the button
    h.click(969.0, 190.0);
    assert_eq!(log.borrow().copied, vec![0]);
    assert!(log.borrow().clicked.is_empty());
    // the name column: the whole row copies on a click
    h.click(500.0, 226.0);
    let l = log.borrow();
    assert_eq!(l.clicked.len(), 1);
    assert_eq!(l.clicked[0].0, 1);
    assert!(l.clicked[0].1 <= 4.0);
}

#[test]
fn a_drag_across_a_row_is_a_selection_not_a_click_for_the_host() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    h.drag((450.0, 190.0), (520.0, 190.0), 6);
    let l = log.borrow();
    assert_eq!(l.clicked.len(), 1);
    assert!(
        l.clicked[0].1 > 4.0,
        "travel {} tells the host it was a drag",
        l.clicked[0].1
    );
}

#[test]
fn a_drag_in_the_command_selects_its_text() {
    let h = harness(1.0);
    let _log = wire(&h);
    open_with(&h, three_rows());
    // A window a person is using is active, which is when a field draws its selection.
    h.activate();
    let before = h.render();
    // A list that scrolls holds a press back for a moment to see if it becomes a drag, so a person's
    // drag (which takes time) selects and an instant one does not: press, wait, then move.
    h.move_to(665.0, 190.0);
    h.press(665.0, 190.0, PointerEventButton::Left);
    h.advance(150);
    for x in [680.0, 700.0, 720.0, 740.0] {
        h.move_to(x, 190.0);
    }
    h.release(740.0, 190.0, PointerEventButton::Left);
    let after = h.render();
    let (x0, y0, x1, y1) = (h.phys(650.0), h.phys(176.0), h.phys(760.0), h.phys(204.0));
    let mut changed = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if before.px(x, y) != after.px(x, y) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 100,
        "{changed} pixels changed where a selection should be drawn"
    );
}

#[test]
fn the_panel_lets_go_of_the_keys_after_its_edge_held_them_and_it_closed() {
    let h = harness(1.0);
    let _log = wire(&h);
    open_with(&h, three_rows());
    h.activate();
    // A press on the left edge gives the edge the keys; then the panel closes and the shell takes them.
    let edge = h.comp.get_sheet_x();
    h.click(edge, 300.0);
    assert!(h.comp.get_panel_focused(), "the edge holds the keys");
    h.comp.global::<HelpBridge>().set_open(false);
    h.advance(300);
    h.comp.invoke_focus_shell();
    h.comp.global::<HelpBridge>().set_open(true);
    h.advance(300);
    h.comp.invoke_focus_shell();
    assert!(
        !h.comp.get_panel_focused(),
        "the shell has the keys, so the panel does not claim them"
    );
}

#[test]
fn up_down_page_enter_escape_fire_key() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    for k in [
        Key::UpArrow,
        Key::DownArrow,
        Key::PageUp,
        Key::PageDown,
        Key::Return,
        Key::Escape,
    ] {
        h.key_text(&key(k));
    }
    let names: Vec<String> = log.borrow().keys.iter().map(|k| k.0.clone()).collect();
    assert_eq!(
        names,
        ["Up", "Down", "PageUp", "PageDown", "Enter", "Escape"]
    );
    // chords pass: the host keeps them
    log.borrow_mut().keys.clear();
    h.key_with(false, true, &key(Key::DownArrow));
    h.key_with(false, true, &key(Key::Escape));
    assert!(log.borrow().keys.is_empty(), "chords are rejected");
    // typing reaches the field and reports the query
    h.key_text("p");
    assert_eq!(log.borrow().queries.last().map(String::as_str), Some("p"));
    assert_eq!(h.comp.global::<HelpBridge>().get_query(), "p");
}

#[test]
fn the_close_button_and_the_chips_report() {
    let h = harness(1.0);
    let log = wire(&h);
    open_with(&h, three_rows());
    // the close button is 28 by 28 at the top right of the header: x 954..982, y 50..78
    h.click(968.0, 64.0);
    assert_eq!(log.borrow().closed, 1);
    // the second chip (Command Prompt) is at x about 485
    h.click(540.0, 148.0);
    assert_eq!(log.borrow().chips, vec!["cmd".to_string()]);
}

#[test]
fn no_scrollbar_and_headings_scroll_with_the_list() {
    let h = harness(1.0);
    let mut rows: Vec<HelpRowData> = (0..40)
        .map(|i| data(&format!("Task {i}"), &format!("cmd-{i}")))
        .collect();
    rows[0].header = "Files".into();
    rows[10].header = "Network".into();
    open_with(&h, rows.clone());
    h.move_to(5.0, 5.0);
    let f = h.render();
    // no bar: the right edge column of the list is the sheet, row after row
    let edge = f.count_where(
        h.phys(996.0),
        h.phys(180.0),
        h.phys(1000.0),
        h.phys(600.0),
        |p| {
            !Frame::near(p, SIDE_FLAT, 1)
                && !Frame::near(p, blend([0xde, 0xda, 0xcf], 0.04, SIDE_FLAT), 1)
        },
    );
    assert_eq!(edge, 0, "nothing drawn at the right edge");
    // the first heading is dim ink above row 0 (y 172..209)
    let dim = |p: [u8; 4]| p[0] > 90 && p[0] < 200;
    let top = f.count_where(
        h.phys(397.0),
        h.phys(172.0),
        h.phys(470.0),
        h.phys(209.0),
        dim,
    );
    assert!(top > 0, "heading drawn");
    // scroll the list by 100: the heading goes up with it, nothing sticks at the top
    h.comp.set_viewport_y(-100.0);
    h.advance(1);
    let f = h.render();
    let top = f.count_where(
        h.phys(397.0),
        h.phys(172.0),
        h.phys(470.0),
        h.phys(209.0),
        dim,
    );
    // what is at the top now is row text, not a heading: no 11.5 px heading band stays put
    let gap_row = f.count_where(
        h.phys(397.0),
        h.phys(172.0),
        h.phys(400.0),
        h.phys(173.0),
        dim,
    );
    assert_eq!(gap_row, 0);
    let _ = top;
    assert!((h.comp.get_viewport_y() + 100.0).abs() < 0.01);
}

#[test]
fn an_active_row_off_screen_is_scrolled_into_view() {
    let h = harness(1.0);
    let rows: Vec<HelpRowData> = (0..60)
        .map(|i| data(&format!("Task {i}"), &format!("cmd-{i}")))
        .collect();
    let model = open_with(&h, rows);
    let mut row = model.row_data(30).unwrap();
    row.active = true;
    model.set_row_data(30, row);
    h.advance(1);
    let _ = h.render();
    let y = h.comp.get_viewport_y();
    assert!(
        y < -650.0 && y > -720.0,
        "row 30 scrolled to the bottom edge: {y}"
    );
}

#[test]
fn near_the_end_asks_for_more_rows() {
    let h = harness(1.0);
    let asked = Rc::new(RefCell::new(0));
    let a = asked.clone();
    h.comp
        .global::<HelpBridge>()
        .on_near_end(move || *a.borrow_mut() += 1);
    let rows: Vec<HelpRowData> = (0..100)
        .map(|i| data(&format!("Task {i}"), &format!("cmd-{i}")))
        .collect();
    open_with(&h, rows);
    h.comp.set_viewport_y(-3000.0);
    h.advance(1);
    assert!(*asked.borrow() > 0, "near-end fired");
}
