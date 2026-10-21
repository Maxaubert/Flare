//! UI tests of the Settings look at the eight scales (M7 Task 45); headless only.
//!
//! The pages are the real page models, laid out by `flare_settings::layout` and drawn by
//! `SettingsPreview` (see `settings_layout.rs` for the shared `Scene`). What is asserted is read from the
//! rendered frame and held to the model's rectangles and to Prism Terminal's measured numbers.

use std::cell::RefCell;
use std::rc::Rc;

use flare_settings::layout::place_menu_at;
use flare_settings::layout::{BlockKind as LayoutBlock, SectionLayout};
use flare_settings::page::{BlockModel, Control, RowModel};
use flare_settings::pages::PAGES;
use flare_settings::picker::{POPOVER_W, Rect, place_popover_at};
use slint::{ComponentHandle, Model};

use super::settings_layout::{Scene, row_blocks};
use crate::SettingsBridge;
use crate::ui_harness::SCALES;

type Px4 = [u8; 4];

/// The reference scales of the screenshots: 100, 150 and 225 percent.
const REFERENCE: [f32; 3] = [1.0, 1.5, 2.25];

fn tall(sc_page: &str, scale: f32, w: u32) -> Scene {
    // The frame is as tall as the content, so every row is on screen.
    let probe = Scene::new(sc_page, scale, w, 604);
    let h = probe.layout.content_h.ceil() as u32 + 1;
    drop(probe);
    Scene::new(sc_page, scale, w, h)
}

/// The row model and its layout box, in page order.
fn rows_of(sc: &Scene) -> Vec<(usize, &RowModel, &flare_settings::layout::RowLayout)> {
    let mut out = Vec::new();
    for (si, (s, l)) in sc
        .model
        .sections
        .iter()
        .zip(&sc.layout.sections)
        .enumerate()
    {
        for (b, rl) in s.blocks.iter().zip(&l.rows) {
            if let BlockModel::Row(r) = b {
                out.push((si, r, rl));
            }
        }
    }
    out
}

/// True when a pixel differs from `base` by more than `tol` in some channel.
fn differs(p: Px4, base: Px4, tol: u8) -> bool {
    p.iter()
        .zip(base.iter())
        .take(3)
        .any(|(a, b)| a.abs_diff(*b) > tol)
}

fn first_of_kind(
    sc: &Scene,
    pick: impl Fn(&Control) -> bool,
) -> Option<(usize, &RowModel, &flare_settings::layout::RowLayout)> {
    rows_of(sc).into_iter().find(|(_, r, _)| pick(&r.control))
}

#[test]
fn the_wall_cards_are_one_height_in_rows_of_the_right_count() {
    for s in REFERENCE {
        for w in [900_u32, 1000, 1600] {
            let sc = tall("appearance", s, w);
            let (si, bi) = sc
                .layout
                .sections
                .iter()
                .enumerate()
                .find_map(|(si, sec)| {
                    sec.rows
                        .iter()
                        .position(|r| r.block == LayoutBlock::Wall)
                        .map(|bi| (si, bi))
                })
                .expect("the Appearance page holds the wall");
            let sec = &sc.layout.sections[si];
            let block = &sec.rows[bi];
            // The model: 196 wide, 126 high, 12 apart; the right number of columns; two rows visible.
            let cols = sc
                .wall
                .positions
                .iter()
                .take_while(|p| p.1 == 0.0)
                .count()
                .max(1);
            let inner = sec.panel_w - 2.0 - 32.0;
            let want_cols = (((inner + 12.0) / (172.0 + 12.0)).floor() as usize).max(1);
            assert_eq!(cols, want_cols, "{w} @{s}: columns");
            assert_eq!(sc.wall.cols, want_cols);
            for (i, (x, y)) in sc.wall.positions.iter().enumerate() {
                assert_eq!(
                    *x,
                    sc.wall.positions[i % cols].0,
                    "{w} @{s}: card {i} x is its column's"
                );
                assert_eq!(
                    *y,
                    (i / cols) as f64 * 138.0,
                    "{w} @{s}: card {i} y (126 high, 12 gap)"
                );
            }
            assert_eq!(sc.wall.clip_h, 268.0, "{w} @{s}: two rows show (268 clip)");
            assert!(
                (block.h - (268.0 + 66.0)).abs() <= 1.0 / sc.scale,
                "{w} @{s}: the wall block is the clip plus 66, got {}",
                block.h
            );
            // The frame. The cards are told apart by their own ground (the panel's fill is the page's
            // ground in these tests, so a card that shares it cannot be found): the first card of the first
            // row and the first card of the second row whose ground differs from the panel's.
            let f = sc.render();
            let cards = sc.h.comp.global::<SettingsBridge>().get_wall_cards();
            let block_left = sc.row_left(sec);
            let (bx0, by0) = (sc.dev(block_left), sc.dev(block.y));
            let fill = f.px(bx0 + sc.dev(4.0), by0 + sc.dev(3.0));
            let ground_of = |i: usize| {
                let c = cards.row_data(i).expect("a card");
                [c.bg.red(), c.bg.green(), c.bg.blue(), 255]
            };
            let pick = |range: std::ops::Range<usize>| {
                range
                    .clone()
                    .find(|&i| differs(ground_of(i), fill, 2))
                    .unwrap_or_else(|| {
                        panic!("{w} @{s}: cards {range:?} all share the panel's colour")
                    })
            };
            let hair = sc.hair_dev();
            let i0 = pick(0..cols);
            let (cx, cy) = sc.wall.positions[i0];
            // The search box holds the card wherever the block pads it (PT: 16 in; see the next test),
            // and stops short of the next card in the row (12 apart).
            let left = block_left + cx;
            let ny = block.y + 14.0 + cy;
            // Its first column: the first of the card's ground on a row below the text.
            let probe_y = sc.dev(ny + 80.0);
            let start = (sc.dev(left)..sc.dev(left + 40.0))
                .find(|&x| f.px(x, probe_y) == ground_of(i0))
                .unwrap_or_else(|| panic!("{w} @{s}: card {i0}'s ground is on screen"));
            let bb0 = f
                .bbox_where(
                    start,
                    sc.dev(ny - 8.0),
                    start + sc.dev(sc.wall.widths[i0]),
                    sc.dev(ny + 126.0 + 8.0),
                    |p| !differs(p, ground_of(i0), 0),
                )
                .unwrap_or_else(|| panic!("{w} @{s}: card {i0} paints its own ground"));
            let card_w = bb0.2 - bb0.0 + 1;
            // A card's ground is 196 minus its two hairlines wide, 92 high.
            assert!(
                card_w.abs_diff(sc.dev(sc.wall.widths[i0]) - 2 * hair) <= 1,
                "{w} @{s}: the card's ground is {card_w} device px wide, wanted {}",
                sc.dev(sc.wall.widths[i0]) - 2 * hair
            );
            let ground_h = bb0.3 - bb0.1 + 1;
            assert!(
                ground_h.abs_diff(sc.dev(92.0)) <= 2,
                "{w} @{s}: the card's ground is {ground_h} high, wanted 92"
            );
            // 14 under the block's top, then its 1 device pixel border.
            let dy = f64::from(bb0.1) / sc.scale - (ny + sc.hair());
            // (Flare puts the 14 under the block's own top rule, Prism Terminal's counts the rule inside it:
            // at most one hairline lower; a documented difference for docs/screenshots/m7/README.md.)
            assert!(
                dy.abs() <= 1.0 + sc.hair(),
                "{w} @{s}: the first card is {dy} css px off the reference's 14 under the block's top"
            );
            // The first card of the second row is 138 lower; its ground is the same height.
            if sc.wall.positions.len() > cols {
                let j = pick(cols..(2 * cols).min(sc.wall.positions.len()));
                let (jx, jy) = sc.wall.positions[j];
                let (qx, qy) = (block_left + jx, block.y + 14.0 + jy);
                let start1 = (sc.dev(qx)..sc.dev(qx + 40.0))
                    .find(|&x| f.px(x, sc.dev(qy + 80.0)) == ground_of(j))
                    .unwrap_or_else(|| panic!("{w} @{s}: card {j}'s ground is on screen"));
                let bb1 = f
                    .bbox_where(
                        start1,
                        sc.dev(qy - 8.0),
                        start1 + sc.dev(sc.wall.widths[j]),
                        sc.dev(qy + 126.0 + 8.0),
                        |p| !differs(p, ground_of(j), 0),
                    )
                    .unwrap_or_else(|| panic!("{w} @{s}: card {j} paints"));
                assert!(
                    (bb1.3 - bb1.1 + 1).abs_diff(ground_h) <= 1,
                    "{w} @{s}: every card is one height"
                );
                let shift = (f64::from(bb1.1) - f64::from(bb0.1)) / sc.scale - (jy - cy);
                assert!(
                    shift.abs() <= 1.0,
                    "{w} @{s}: the second row is {shift} off 138 lower"
                );
            }
        }
    }
}

/// Prism Terminal pads the wall 16 px from the panel's edge (`WALL_PAD`, `termCard` x 305 in a row at 289).
/// Slint draws the first card flush against the panel's left edge, 16 px short.
#[test]
fn the_wall_cards_start_16_in_from_the_blocks_edge() {
    for s in REFERENCE {
        let sc = tall("appearance", s, 1000);
        let sec = sc
            .layout
            .sections
            .iter()
            .find(|x| x.rows.iter().any(|r| r.block == LayoutBlock::Wall))
            .expect("the wall");
        let block = sec
            .rows
            .iter()
            .find(|r| r.block == LayoutBlock::Wall)
            .expect("the block");
        let f = sc.render();
        let cards = sc.h.comp.global::<SettingsBridge>().get_wall_cards();
        let c = cards.row_data(0).expect("a card");
        let ground = [c.bg.red(), c.bg.green(), c.bg.blue(), 255];
        let left = sc.row_left(sec);
        // The card's own edge is the first column in its row that is neither the panel's nor the ring.
        let start = (sc.dev(left)..sc.dev(left + 60.0))
            .find(|&x| f.px(x, sc.dev(block.y + 14.0 + 80.0)) == ground)
            .expect("the first card paints");
        let dx = f64::from(start) / sc.scale - left - sc.hair();
        assert!(
            (dx - 16.0).abs() <= 1.0,
            "@{s}: the first card starts {dx} css px in from the block's edge, the reference's is 16"
        );
    }
}

#[test]
fn the_page_header_and_labels_have_the_right_sizes() {
    for s in SCALES {
        let sc = tall("appearance", s, 1000);
        let f = sc.render();
        let ground = f.px(sc.dev(sc.rail_w() + 4.0), sc.dev(4.0));
        // The header: 24 px bold, one line of "Appearance" (cap height to descender).
        let hx = sc.dev(sc.rail_w() + sc.layout.pad_x);
        let hb = f
            .bbox_where(
                hx,
                sc.dev(sc.layout.header_y),
                hx + sc.dev(400.0),
                sc.dev(sc.layout.header_y + sc.layout.header_h),
                |p| differs(p, ground, 40),
            )
            .expect("the header draws");
        let hh = f64::from(hb.3 - hb.1 + 1);
        assert!(
            hh >= 0.55 * 24.0 * sc.scale && hh <= 1.3 * 24.0 * sc.scale,
            "@{s}: the header's glyph rows are {hh} device px for 24 px text"
        );
        // A row's label (13 px) and subtext (11.5 px), the first row of the first panel.
        let (_, r, rl) = rows_of(&sc)[0];
        let sec: &SectionLayout = &sc.layout.sections[0];
        let left = sc.row_left(sec);
        let fill = sc.panel_fill(&f, sec);
        let x0 = sc.dev(left + 60.0);
        let x1 = x0 + sc.dev(220.0);
        let label = f
            .bbox_where(x0, sc.dev(rl.y + 7.0), x1, sc.dev(rl.y + 29.0), |p| {
                differs(p, fill, 40)
            })
            .unwrap_or_else(|| panic!("@{s}: the label of {} draws", r.id));
        let lh = f64::from(label.3 - label.1 + 1);
        assert!(
            lh >= 0.5 * 13.0 * sc.scale && lh <= 1.3 * 13.0 * sc.scale,
            "@{s}: the label's glyph rows are {lh} device px for 13 px text"
        );
        let sub = f
            .bbox_where(x0, sc.dev(rl.y + 30.0), x1, sc.dev(rl.y + 52.0), |p| {
                differs(p, fill, 25)
            })
            .unwrap_or_else(|| panic!("@{s}: the subtext of {} draws", r.id));
        let sh = f64::from(sub.3 - sub.1 + 1);
        assert!(
            sh >= 0.35 * 11.5 * sc.scale && sh <= 1.3 * 11.5 * sc.scale,
            "@{s}: the subtext's glyph rows are {sh} device px for 11.5 px text"
        );
        // The label is larger than the subtext, the header larger than the label.
        assert!(
            hh > lh && lh >= sh * 0.9,
            "@{s}: header {hh} > label {lh} >= subtext {sh}"
        );
    }
}

#[test]
fn fractional_scales_never_blur_a_hairline() {
    for s in [1.25_f32, 1.75, 2.25, 2.5] {
        let sc = tall("appearance", s, 1000);
        let f = sc.render();
        let t = sc.hair_dev();
        let sec = sc
            .layout
            .sections
            .iter()
            .find(|x| x.rows.len() >= 3 && x.rows.iter().all(|r| r.block == LayoutBlock::Row))
            .expect("a section of plain rows");
        let fill = sc.panel_fill(&f, sec);
        let left = sc.dev(sc.row_left(sec));
        // A row's top hairline, in the stretch after the tile (x from 80 to 160 in) where no text sits.
        for ri in 1..sec.rows.len().min(3) {
            let y0 = sc.dev(sec.rows[ri].y);
            let xs = (left + sc.dev(80.0))..(left + sc.dev(160.0));
            let line = f.px(xs.start, y0);
            assert!(line != fill, "@{s} row {ri}: the hairline is drawn");
            for x in xs.clone() {
                for k in 0..t {
                    assert_eq!(
                        f.px(x, y0 + k),
                        line,
                        "@{s} row {ri}: the rule is one colour ({t} device rows)"
                    );
                }
                assert_eq!(
                    f.px(x, y0 - 1),
                    fill,
                    "@{s} row {ri}: no blend above the rule"
                );
                assert_eq!(
                    f.px(x, y0 + t),
                    fill,
                    "@{s} row {ri}: no blend below the rule"
                );
            }
        }
        // The panel's own top border has the same thickness and a single colour.
        let py = sc.dev(sec.panel_y);
        let xs = (left + sc.dev(80.0))..(left + sc.dev(160.0));
        let ground = f.px(xs.start, py - 1);
        let border = f.px(xs.start, py);
        assert!(border != ground, "@{s}: the panel border is drawn");
        for x in xs {
            for k in 0..t {
                assert_eq!(f.px(x, py + k), border, "@{s}: the border is one colour");
            }
            assert_eq!(f.px(x, py - 1), ground, "@{s}: no blend above the border");
        }
    }
}

type Anchors = Rc<RefCell<Vec<(String, [f32; 4])>>>;

/// Clicks the control at the right of row `rl` (its centre, `from_right` css px in from the control edge).
fn click_control(sc: &Scene, rl: &flare_settings::layout::RowLayout, from_right: f64) {
    let right = sc.rail_w() + rl.x + rl.w - 16.0;
    sc.h.click((right - from_right) as f32, (rl.y + rl.h / 2.0) as f32);
}

fn whole3(v: f64, scale: f64) -> bool {
    (v * scale - (v * scale).round()).abs() < 1e-3
}

#[test]
fn the_select_menu_and_popover_sit_on_the_pixel_grid() {
    // Every violation at every scale is collected, so one failure lists them all.
    let mut bad: Vec<String> = Vec::new();
    for s in SCALES {
        let sc64 = f64::from(s);
        let mut check = |what: String, v: f64| {
            if !whole3(v, sc64) {
                bad.push(format!("@{s}: {what} = {v} is not on a device pixel"));
            }
        };
        let sc = tall("terminal", s, 1000);
        let anchors: Anchors = Rc::default();
        let b = sc.h.comp.global::<SettingsBridge>();
        let a = anchors.clone();
        b.on_select_open_request(move |id, x, y, w, h| {
            a.borrow_mut().push((id.to_string(), [x, y, w, h]));
        });
        let (_, shell, rl) = first_of_kind(&sc, |c| matches!(c, Control::Select { .. }))
            .expect("the Terminal page has a select");
        click_control(&sc, rl, 20.0);
        sc.h.advance(0);
        let got = anchors.borrow().last().cloned();
        let (id, [ax, ay, aw, ah]) =
            got.unwrap_or_else(|| panic!("@{s}: the trigger reports its box"));
        assert_eq!(id, shell.id);
        for (what, v) in [("x", ax), ("y", ay), ("w", aw), ("h", ah)] {
            check(format!("the trigger's {what}"), f64::from(v));
        }
        // The menu the glue opens from that anchor (right edge to the trigger's, 4 below it).
        let options = match &shell.control {
            Control::Select { options, .. } => options.len() as f64,
            _ => unreachable!(),
        };
        let window = (sc.frame_w, sc.frame_h);
        let (mx, my, mh) = place_menu_at(
            Rect {
                x: f64::from(ax),
                y: f64::from(ay),
                w: f64::from(aw),
                h: f64::from(ah),
            },
            f64::from(aw),
            options * 28.0 + 8.0,
            window,
            320.0,
            sc64,
        );
        for (what, v) in [
            ("x", mx),
            ("y", my),
            ("h", mh),
            ("right", mx + f64::from(aw)),
        ] {
            check(format!("the menu's {what}"), v);
        }
        // Drawn: the menu's border is crisp, one colour, `hair` device px thick, at the device pixels asked.
        b.set_select_options(slint::ModelRc::new(slint::VecModel::from(vec![
            crate::SetOption {
                id: "a".into(),
                name: "Alpha".into(),
                face: "".into(),
            },
            crate::SetOption {
                id: "b".into(),
                name: "Beta".into(),
                face: "".into(),
            },
        ])));
        b.set_select_value("a".into());
        b.set_select_x(mx as f32);
        b.set_select_y(my as f32);
        b.set_select_w(aw);
        b.set_select_h(mh as f32);
        b.set_select_open(true);
        sc.h.advance(0);
        let f = sc.render();
        let t = sc.hair_dev();
        let (x0, y0) = (sc.dev(mx), sc.dev(my));
        let mid = y0 + sc.dev(mh) / 2;
        let edge = f.px(x0, mid);
        let outside = f.px(x0.saturating_sub(1), mid);
        assert!(
            edge != outside,
            "@{s}: the menu's left edge starts at its device pixel"
        );
        for k in 0..t {
            assert_eq!(
                f.px(x0 + k, mid),
                edge,
                "@{s}: the menu's border is one colour"
            );
        }
        assert!(
            f.px(x0 + t, mid) != edge,
            "@{s}: the menu's border is {t} device px thick, not blurred wider"
        );
        b.set_select_open(false);

        // The popover: placed under the swatch (Appearance has a colour field).
        let sc2 = tall("appearance", s, 1000);
        let anchors: Anchors = Rc::default();
        let b2 = sc2.h.comp.global::<SettingsBridge>();
        let a = anchors.clone();
        b2.on_colour_swatch(move |id, x, y, w, h| {
            a.borrow_mut().push((id.to_string(), [x, y, w, h]));
        });
        let (_, _, rl2) = first_of_kind(&sc2, |c| matches!(c, Control::Colour { .. }))
            .expect("the Appearance page has a colour row");
        click_control(&sc2, rl2, 18.0);
        sc2.h.advance(0);
        let got = anchors.borrow().last().cloned();
        let (_, [sx, sy, sw, sh]) =
            got.unwrap_or_else(|| panic!("@{s}: the swatch reports its box"));
        for (what, v) in [("x", sx), ("y", sy), ("w", sw), ("h", sh)] {
            check(format!("the swatch's {what}"), f64::from(v));
        }
        let (px, py) = place_popover_at(
            Rect {
                x: f64::from(sx),
                y: f64::from(sy),
                w: f64::from(sw),
                h: f64::from(sh),
            },
            (POPOVER_W, 254.0),
            (sc2.frame_w, sc2.frame_h),
            sc64,
        );
        let snap = |v: f64| (v * sc64).round() / sc64;
        for (what, v) in [
            ("x", px),
            ("y", py),
            ("right", px + snap(POPOVER_W)),
            ("bottom", py + snap(254.0)),
        ] {
            check(format!("the popover's {what}"), v);
        }
    }
    assert!(
        bad.is_empty(),
        "the anchors Slint reports (and so the menu and popover Rust places from them) are off the device grid:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_dictation_page_draws_its_cards() {
    for s in REFERENCE {
        let sc = Scene::new("dictation", s, 1000, 604);
        // The model: the master card leads, and the page's other cards follow, rows in each.
        assert!(
            sc.model.sections.len() >= 2,
            "@{s}: Dictation has more than one card ({})",
            sc.model.sections.len()
        );
        assert_eq!(
            sc.model.sections[0].id, "dictation",
            "@{s}: the master card leads the page"
        );
        let rows: Vec<&str> = sc
            .model
            .sections
            .iter()
            .flat_map(|x| x.blocks.iter())
            .filter_map(|b| match b {
                BlockModel::Row(r) => Some(r.id),
                _ => None,
            })
            .collect();
        assert!(
            rows.contains(&"dictation-enabled"),
            "@{s}: the master row is drawn ({rows:?})"
        );
        // The rail lists all six pages, Dictation fourth, About pinned last.
        let rail = sc.h.comp.global::<SettingsBridge>().get_rail();
        let ids: Vec<String> = rail.iter().map(|r| r.id.to_string()).collect();
        let want: Vec<String> = PAGES.iter().map(|p| p.id.to_string()).collect();
        assert_eq!(ids, want, "@{s}: the rail lists the six pages");
        assert_eq!(ids[3], "dictation");
        let chosen: Vec<String> = rail
            .iter()
            .filter(|r| r.chosen)
            .map(|r| r.id.to_string())
            .collect();
        assert_eq!(
            chosen,
            vec!["dictation".to_string()],
            "@{s}: Dictation is the chosen item"
        );
        // The frame: the rail item draws a label, and the note draws its words in the panel.
        let f = sc.render();
        let item = rail.row_data(3).expect("the dictation item");
        let ground = f.px(sc.dev(4.0), sc.dev(4.0));
        let label = f.bbox_where(
            sc.dev(12.0 + 10.0 + 17.0 + 11.0),
            sc.dev(f64::from(item.y)),
            sc.dev(230.0),
            sc.dev(f64::from(item.y) + 36.0),
            |p| {
                differs(
                    p,
                    f.px(
                        sc.dev(12.0 + 10.0 + 17.0 + 11.0),
                        sc.dev(f64::from(item.y)) + 1,
                    ),
                    20,
                )
            },
        );
        assert!(
            label.is_some(),
            "@{s}: the Dictation item draws its label (ground {ground:?})"
        );
        let sec = &sc.layout.sections[0];
        let fill = sc.panel_fill(&f, sec);
        let r = &sec.rows[0];
        let note = f.bbox_where(
            sc.dev(sc.row_left(sec) + 14.0),
            sc.dev(r.y),
            sc.dev(sc.row_left(sec) + r.w),
            sc.dev(r.y + r.h),
            |p| differs(p, fill, 25),
        );
        assert!(note.is_some(), "@{s}: the first card's row draws its words");
    }
}

#[test]
fn every_row_has_a_label_a_tile_and_a_control_where_it_should() {
    for s in REFERENCE {
        for page in ["appearance", "terminal", "agents", "diagnostics", "about"] {
            let sc = tall(page, s, 1000);
            let drawn: Vec<String> =
                sc.h.comp
                    .global::<SettingsBridge>()
                    .get_sections()
                    .iter()
                    .flat_map(|x| x.rows.iter().collect::<Vec<_>>())
                    .filter(|r| r.block == crate::BlockKind::Row)
                    .map(|r| r.id.to_string())
                    .collect();
            let model: Vec<String> = rows_of(&sc)
                .iter()
                .map(|(_, r, _)| r.id.to_string())
                .collect();
            assert_eq!(
                drawn, model,
                "{page} @{s}: the drawn row ids equal the model's"
            );
            let f = sc.render();
            let _ = row_blocks(&sc.layout);
            for (si, r, rl) in rows_of(&sc) {
                let sec = &sc.layout.sections[si];
                let fill = sc.panel_fill(&f, sec);
                let left = sc.row_left(sec);
                let (top, bottom) = (rl.y + 3.0, rl.y + rl.h - 3.0);
                // The tile.
                let tile = f.bbox_where(
                    sc.dev(left + 6.0),
                    sc.dev(top),
                    sc.dev(left + 58.0),
                    sc.dev(bottom),
                    |p| p != fill,
                );
                let tile = tile.unwrap_or_else(|| panic!("{page} @{s}: {} draws its tile", r.id));
                let side = sc.dev(32.0);
                assert!(
                    (tile.2 - tile.0 + 1).abs_diff(side) <= 1,
                    "{page} @{s}: {} tile width",
                    r.id
                );
                // The label sits right of the tile, in the label column.
                let label = f.bbox_where(
                    sc.dev(left + 60.0),
                    sc.dev(rl.y + 7.0),
                    sc.dev(left + 60.0 + 220.0),
                    sc.dev(rl.y + 29.0),
                    |p| differs(p, fill, 40),
                );
                assert!(label.is_some(), "{page} @{s}: {} draws its label", r.id);
                // A control (or buttons) at the right edge, inside the row.
                let wants_control = !matches!(r.control, Control::None) || !r.buttons.is_empty();
                if wants_control {
                    let right = left + rl.w - 1.0 - 16.0;
                    let ctl = f.bbox_where(
                        sc.dev(right - 150.0),
                        sc.dev(top),
                        sc.dev(right + 4.0),
                        sc.dev(bottom),
                        |p| differs(p, fill, 6),
                    );
                    let ctl =
                        ctl.unwrap_or_else(|| panic!("{page} @{s}: {} draws its control", r.id));
                    assert!(
                        f64::from(ctl.2) <= f64::from(sc.dev(right + 4.0)),
                        "{page} @{s}: {} control stays in the row",
                        r.id
                    );
                }
            }
        }
    }
}
