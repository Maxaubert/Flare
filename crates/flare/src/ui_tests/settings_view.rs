//! UI tests of the settings view and its pages (M7 Task 40); headless only.
//!
//! `SettingsPreview` is the whole window (1000 by 604 css px): the view fills it and the overlays sit
//! above it. Every test sets `SettingsBridge` (the one global the view reads) and clicks by css
//! coordinates computed from the preview's out properties; callbacks are recorded into a log.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::ui_harness::{Harness, SCALES};
use crate::{
    BlockKind, SetButton, SetHit, SetKind, SetOption, SetRow, SetSection, SettingsBridge,
    SettingsPreview,
};

type Log = Rc<RefCell<Vec<String>>>;

const ICON: &str = "M4 6h16v12H4z";
const ROW_H: f32 = 58.0;
const TITLE_Y: f32 = 20.0;
const PANEL_Y: f32 = 50.0;

fn opts(ids: &[&str]) -> ModelRc<SetOption> {
    let v: Vec<SetOption> = ids
        .iter()
        .map(|i| SetOption {
            id: SharedString::from(*i),
            name: SharedString::from(i.to_uppercase()),
            face: SharedString::new(),
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

fn row(id: &str, kind: SetKind, i: usize) -> SetRow {
    SetRow {
        id: id.into(),
        icon: ICON.into(),
        label: id.into(),
        sub: "A line of subtext.".into(),
        block: BlockKind::Row,
        kind,
        y: ROW_H * i as f32,
        h: ROW_H,
        ..Default::default()
    }
}

/// One row of each kind the view draws, in a fixed order: `ids()` names them.
fn every_kind() -> Vec<SetRow> {
    let mut v = Vec::new();
    let mut r = row("pill", SetKind::Pill, 0);
    r.value = "claude".into();
    v.push(r);
    let mut r = row("sw", SetKind::Switch, 1);
    r.tap = true;
    v.push(r);
    let mut r = row("seg", SetKind::Segmented, 2);
    r.options = opts(&["a", "b"]);
    r.value = "a".into();
    v.push(r);
    let mut r = row("sel", SetKind::Select, 3);
    r.options = opts(&["x", "y", "z"]);
    r.value = "x".into();
    v.push(r);
    let mut r = row("sl", SetKind::Slider, 4);
    r.num = 5;
    r.num_max = 10;
    r.num_text = "50%".into();
    v.push(r);
    let mut r = row("col", SetKind::Colour, 5);
    r.reset = true;
    r.code = "#FE8F34".into();
    r.colour = slint::Color::from_rgb_u8(0xfe, 0x8f, 0x34);
    v.push(r);
    let mut r = row("sv", SetKind::Save, 6);
    r.save_dirty = true;
    v.push(r);
    let mut r = row("bt", SetKind::None, 7);
    r.buttons = ModelRc::new(VecModel::from(vec![SetButton {
        id: "go".into(),
        label: "Go".into(),
        label_alt: "Gone".into(),
        off: false,
        alt: false,
    }]));
    v.push(r);
    v
}

fn section(rows: Vec<SetRow>, save: bool) -> SetSection {
    let n = rows.len() as f32;
    SetSection {
        id: "sec".into(),
        title: "Section".into(),
        tag: SharedString::new(),
        action_save: save,
        action_dirty: save,
        title_y: TITLE_Y,
        panel_y: PANEL_Y,
        panel_h: n * ROW_H + 2.0,
        rows: ModelRc::new(VecModel::from(rows)),
        ..Default::default()
    }
}

/// A css length snapped to a whole device pixel at `scale`.
fn sn(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// Puts every box on a whole device pixel (the real layout does this in Rust).
fn snapped(sections: Vec<SetSection>, s: f32) -> Vec<SetSection> {
    sections
        .into_iter()
        .map(|mut sec| {
            let rows: Vec<SetRow> = sec
                .rows
                .iter()
                .map(|mut r| {
                    let (y, bottom) = (sn(r.y, s), sn(r.y + r.h, s));
                    r.y = y;
                    r.h = bottom - y;
                    r
                })
                .collect();
            sec.title_y = sn(sec.title_y, s);
            sec.panel_y = sn(sec.panel_y, s);
            sec.panel_h = sn(rows.len() as f32 * ROW_H, s) + 2.0 * hair(s);
            sec.rows = ModelRc::new(VecModel::from(rows));
            sec
        })
        .collect()
}

fn make(scale: f32, sections: Vec<SetSection>) -> (Harness<SettingsPreview>, Log) {
    let sections = snapped(sections, scale);
    let h = Harness::new(scale, 1000, 604, || {
        SettingsPreview::new().expect("the preview builds")
    });
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let b = h.comp.global::<SettingsBridge>();
    b.set_page_title("Appearance".into());
    b.set_content_h(900.0);
    b.set_sections(ModelRc::new(VecModel::from(sections)));
    let l = log.clone();
    b.on_row_switch(move |id, on| l.borrow_mut().push(format!("switch:{id}:{on}")));
    let l = log.clone();
    b.on_row_choose(move |id, v| l.borrow_mut().push(format!("choose:{id}:{v}")));
    let l = log.clone();
    b.on_row_slide(move |id, _| l.borrow_mut().push(format!("slide:{id}")));
    let l = log.clone();
    b.on_row_button(move |id, bid| l.borrow_mut().push(format!("button:{id}:{bid}")));
    let l = log.clone();
    b.on_row_reset(move |id| l.borrow_mut().push(format!("reset:{id}")));
    let l = log.clone();
    b.on_colour_swatch(move |id, _, _, _, _| l.borrow_mut().push(format!("swatch:{id}")));
    let l = log.clone();
    b.on_select_open_request(move |id, _, _, _, _| l.borrow_mut().push(format!("select:{id}")));
    let l = log.clone();
    b.on_save_clicked(move |id| l.borrow_mut().push(format!("save:{id}")));
    let l = log.clone();
    b.on_hit_chosen(move |i| l.borrow_mut().push(format!("hit:{i}")));
    let l = log.clone();
    b.on_select_chosen(move |id| l.borrow_mut().push(format!("chosen:{id}")));
    let l = log.clone();
    b.on_picker_sv(move |_, _, _| l.borrow_mut().push("sv".to_string()));
    h.advance(0);
    (h, log)
}

/// The panel's absolute left and right edges (css px) and the right edge controls end at.
fn edges(h: &Harness<SettingsPreview>) -> (f32, f32, f32) {
    let left = h.comp.get_pane_x() + h.comp.get_content_x();
    let right = left + h.comp.get_content_w();
    (left, right, right - hair(h.scale()) - 16.0)
}

/// The vertical centre of row `i` (css px).
fn centre(i: usize) -> f32 {
    PANEL_Y + 1.0 + ROW_H * i as f32 + ROW_H / 2.0
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[test]
fn the_view_draws_every_kind_of_row_without_overlap() {
    for s in SCALES {
        let (h, _) = make(s, vec![section(every_kind(), false)]);
        let (left, right, ctl_right) = edges(&h);
        let f = h.render();
        let dev = |c: f32| (c * s).round() as u32;
        let hair_x = dev(left + 100.0);
        for i in 0..8usize {
            let top = sn(PANEL_Y, s) + hair(s) + sn(ROW_H * i as f32, s);
            let at = f.px(hair_x, dev(top) + if i == 0 { 1 } else { 0 });
            let below = f.px(hair_x, dev(top + 6.0));
            if i == 0 {
                assert_eq!(at, below, "scale {s}: the first row has no top rule");
            } else {
                assert_ne!(at, below, "scale {s}: row {i} starts with a hairline");
            }
            // The row's control draws inside the row: something differs from the bare panel at the right.
            let bare = f.px(dev(left + 100.0), dev(top + 6.0));
            let drawn = f.count_where(
                dev(ctl_right - 120.0),
                dev(top + 4.0),
                dev(ctl_right),
                dev(top + ROW_H - 4.0),
                |p| !Frame::near(p, bare, 2),
            ) > 0;
            assert!(drawn, "scale {s}: row {i} draws a control at its right");
        }
        // The panel's border closes under the last row, inside the panel's own height.
        let bottom = sn(PANEL_Y, s) + sn(8.0 * ROW_H, s) + 2.0 * hair(s);
        let edge = f.px(dev(left + 100.0), dev(bottom - hair(s)));
        let inside = f.px(dev(left + 100.0), dev(bottom - 6.0));
        assert_ne!(edge, inside, "scale {s}: the panel's bottom border");
        assert!(right > left);
    }
}

use crate::ui_harness::Frame;

#[test]
fn the_events_carry_the_row_id() {
    for s in [1.0f32, 2.0] {
        let (h, log) = make(s, vec![section(every_kind(), true)]);
        let (left, right, ctl_right) = edges(&h);
        // (row index, x to click, expected log line)
        let cases: [(usize, f32, &str); 7] = [
            (1, ctl_right - 8.0, "switch:sw:true"),
            (2, ctl_right - 12.0, "choose:seg:b"),
            (3, ctl_right - 20.0, "select:sel"),
            (4, ctl_right - 218.0 + 9.0 + 150.0, "slide:sl"),
            (5, ctl_right - 10.0, "swatch:col"),
            (6, ctl_right - 8.0, "save:sv"),
            (7, ctl_right - 8.0, "button:bt:go"),
        ];
        for (i, x, want) in cases {
            h.click(x, centre(i));
            h.advance(0);
            let got = take(&log);
            assert_eq!(got, vec![want.to_string()], "scale {s}: row {i}");
        }
        // The Reset link sits left of the colour field (118 wide) with a gap of 10.
        h.click(ctl_right - 118.0 - 10.0 - 8.0, centre(5));
        assert_eq!(
            take(&log),
            vec!["reset:col".to_string()],
            "scale {s}: reset"
        );
        // The heading's Save button is right aligned in the heading.
        h.click(right - 8.0, TITLE_Y + 10.0);
        assert_eq!(
            take(&log),
            vec!["save:sec".to_string()],
            "scale {s}: heading save"
        );
        assert!(left < right);
    }
}

#[test]
fn a_tap_row_toggles_its_switch_from_anywhere_but_the_control() {
    for s in [1.0f32, 2.0] {
        let (h, log) = make(s, vec![section(every_kind(), false)]);
        let (left, _, ctl_right) = edges(&h);
        // On the label: the row's tap toggles the switch.
        h.click(left + 80.0, PANEL_Y + 1.0 + ROW_H + 18.0);
        h.advance(0);
        assert_eq!(
            take(&log),
            vec!["switch:sw:true".to_string()],
            "scale {s}: label"
        );
        // On the control: exactly one toggle (the control takes the press, the row does not).
        h.click(ctl_right - 8.0, centre(1));
        h.advance(0);
        assert_eq!(
            take(&log),
            vec!["switch:sw:true".to_string()],
            "scale {s}: control"
        );
        // A row that is not tap (the pill row) does nothing.
        h.click(left + 80.0, PANEL_Y + 1.0 + 18.0);
        assert!(take(&log).is_empty(), "scale {s}: a plain row");
    }
}

#[cfg_attr(
    not(windows),
    ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
)]
#[test]
fn search_mode_shows_the_results_panel_in_place_of_the_sections() {
    let (h, log) = make(1.0, vec![section(every_kind(), false)]);
    let hits: Vec<SetHit> = (0..2)
        .map(|i| SetHit {
            id: format!("h{i}").into(),
            icon: ICON.into(),
            label: format!("Setting {i}").into(),
            sub: "A line.".into(),
            r#where: "Terminal".into(),
            selected: false,
            y: 58.0 * i as f32,
        })
        .collect();
    {
        let b = h.comp.global::<SettingsBridge>();
        b.set_searching(true);
        b.set_hits(ModelRc::new(VecModel::from(hits)));
        b.set_hits_h(2.0 * 58.0 + 2.0);
        b.set_hit_cursor(-1);
        b.set_status_text("2 results".into());
    }
    h.advance(0);
    let left = h.comp.get_pane_x() + h.comp.get_content_x();
    let top = h.comp.get_content_top();
    // A click on the second hit chooses it.
    h.click(left + 100.0, top + 1.0 + 58.0 + 29.0);
    assert_eq!(take(&log), vec!["hit:1".to_string()]);
    // The sections are gone: the switch's spot is just the results panel, so nothing fires.
    let right = left + h.comp.get_content_w();
    h.click(right - 1.0 - 16.0 - 8.0, PANEL_Y + 1.0 + ROW_H + 29.0);
    let got = take(&log);
    assert!(
        got.iter().all(|l| !l.starts_with("switch")),
        "no section row answers in search mode: {got:?}"
    );
}

#[test]
fn the_overlays_are_not_clipped_by_the_pane() {
    let (h, log) = make(1.0, vec![section(every_kind(), false)]);
    {
        let b = h.comp.global::<SettingsBridge>();
        // The Select menu sits over the rail (x 20 to 188 is left of the pane at 244).
        b.set_select_options(opts(&["x", "y", "z"]));
        b.set_select_value("x".into());
        b.set_select_x(20.0);
        b.set_select_y(150.0);
        b.set_select_w(168.0);
        b.set_select_h(3.0 * 28.0 + 10.0);
        b.set_select_open(true);
    }
    h.advance(0);
    h.click(20.0 + 84.0, 150.0 + 20.0);
    assert_eq!(
        take(&log),
        vec!["chosen:x".to_string()],
        "the menu over the rail takes the click"
    );
    {
        let b = h.comp.global::<SettingsBridge>();
        b.set_select_open(false);
        // The colour popover over the rail as well.
        b.set_picker_x(20.0);
        b.set_picker_y(20.0);
        b.set_picker_label("Background".into());
        b.set_picker_sat(50.0);
        b.set_picker_val(50.0);
        b.set_picker_alpha(1.0);
        b.set_picker_alpha_on(true);
        b.set_picker_format("HEX".into());
        b.set_picker_open(true);
    }
    h.advance(0);
    h.click(20.0 + 100.0, 20.0 + 80.0);
    assert!(
        take(&log).contains(&"sv".to_string()),
        "the popover over the rail takes the press"
    );
}

#[test]
fn the_section_heading_starts_2_px_inside_the_panel_edge() {
    // PT SettingsSection: the heading row is `mx-0.5`, so its text starts 2 css px right of the
    // panel's left edge (measured: heading x 290, panel x 288).
    for s in SCALES {
        let (h, _) = make(s, vec![section(every_kind(), false)]);
        // No page title: the preview's heading sits where the header would be.
        h.comp.global::<SettingsBridge>().set_page_title("".into());
        h.advance(0);
        let (left, _, _) = edges(&h);
        let f = h.render();
        let dev = |c: f32| (c * s).round() as u32;
        // The panel's top: the first pixel down the column 300 px in that leaves the ground.
        let ground = f.px(dev(left + 300.0), 1);
        let col = dev(left + 300.0);
        let top = (0..f.h)
            .find(|&y| !Frame::near(f.px(col, y), ground, 3))
            .expect("the panel is drawn");
        // The heading's ink: the band 26 css px above the panel, from 8 px left of the panel.
        let ink = f
            .bbox_where(
                dev(left - 8.0),
                top - dev(26.0),
                dev(left + 200.0),
                top - dev(3.0),
                |p| p[0] > 110,
            )
            .expect("the heading is drawn");
        let dx = ink.0 as f32 / s - left;
        assert!(
            (1.5..=3.5).contains(&dx),
            "scale {s}: the heading's ink starts {dx:.2} css px from the panel's edge"
        );
    }
}

/// `Px.hair()` in css px: a whole number of device pixels, at least one.
fn hair(scale: f32) -> f32 {
    scale.floor().max(1.0) / scale
}
