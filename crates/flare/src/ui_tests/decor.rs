//! The decor chip on a tab (plugins phase 1), headless: the outlined pill draws before the end, the name gives
//! way to it, it hides where there is no room and on icon tabs, and it never covers the agent's bar at the foot.

use slint::{Color, ModelRc, VecModel};

use crate::menus_ui::{GLYPH_BADGE_PHONE, glyph_path};
use crate::ui_harness::{Frame, Harness};
use crate::{StripGeom, TabRow, TabSidebarPreview, TabStripPreview};

const GROUND: [u8; 4] = [23, 23, 23, 255];
const SCALES: [f32; 3] = [1.0, 1.5, 2.0];

fn ground(p: [u8; 4]) -> bool {
    Frame::near(p, GROUND, 2)
}

fn red(p: [u8; 4]) -> bool {
    p[0] > 150 && p[1] < 80 && p[2] < 80
}

/// Three named tabs of 150 css px, none in front, no names (so only the chip inks the middle of a tab).
fn rows(s: f32) -> Vec<TabRow> {
    (0..3)
        .map(|i| TabRow {
            id: format!("t{i}").into(),
            label: "".into(),
            tip: format!("C:\\t{i}").into(),
            kept: true,
            mode: 0,
            x: (i as f32 * 150.0 * s).round() as i32,
            w: (150.0 * s).round() as i32,
            prog: -1.0,
            ..TabRow::default()
        })
        .collect()
}

/// A progress at half and one phone badge, with room for both.
fn decorate(r: &mut TabRow) {
    r.prog = 0.5;
    r.chip_count = 1;
    r.chip1_cmd = glyph_path(GLYPH_BADGE_PHONE).into();
    r.chip_room = 2;
}

fn strip(s: f32, rows: Vec<TabRow>) -> Harness<TabStripPreview> {
    let h = Harness::new(s, 600, 36, move || {
        let c = TabStripPreview::new().unwrap();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_geom(StripGeom {
            more_x: 0,
            more_w: 0,
            plus_x: (460.0 * s).round() as i32,
            plus_w: (36.0 * s).round() as i32,
            more_count: 0,
            dots: ModelRc::default(),
        });
        c
    });
    h.advance(0);
    h
}

fn px(s: f32, css: f32) -> u32 {
    (css * s).round() as u32
}

/// The ink in tab 1 between its icon slot and its divider.
fn ink_of_tab1(f: &Frame, s: f32) -> Option<(u32, u32, u32, u32)> {
    f.bbox_where(px(s, 150.0 + 30.0), 0, px(s, 150.0 + 146.0), f.h - 2, |p| {
        !ground(p)
    })
}

#[test]
fn the_pill_draws_before_the_end_in_the_tabs_middle() {
    for s in SCALES {
        let bare = strip(s, rows(s)).render();
        assert_eq!(
            ink_of_tab1(&bare, s),
            None,
            "scale {s}: an undecorated tab has no ink there"
        );
        let mut r = rows(s);
        decorate(&mut r[1]);
        let f = strip(s, r).render();
        let (x0, y0, x1, y1) = ink_of_tab1(&f, s).expect("the pill has ink");
        // The end box is 16 px at 10 px from the right edge; the pill is 4 px before it: 8 + 18 + 3 + 14 = 43 wide.
        let right = 150.0 + 150.0 - 10.0 - 16.0 - 4.0;
        assert!(x1.abs_diff(px(s, right) - 1) <= 2, "scale {s}: right {x1}");
        assert!(
            x0.abs_diff(px(s, right - 43.0)) <= 2,
            "scale {s}: left {x0}"
        );
        // 20 px tall on the tab's middle.
        assert!(
            y0 >= px(s, 6.0) && y1 <= px(s, 29.0),
            "scale {s}: {y0}..{y1}"
        );
        // The other tabs are bare.
        assert_eq!(
            f.count_where(px(s, 30.0), 0, px(s, 146.0), f.h - 2, |p| !ground(p)),
            0
        );
        // An unknown amount is still drawn, as dots.
        let mut r = rows(s);
        decorate(&mut r[1]);
        r[1].prog = -2.0;
        assert!(ink_of_tab1(&strip(s, r).render(), s).is_some());
    }
}

#[test]
fn the_name_gives_way_to_the_pill() {
    let long: slint::SharedString = "abcdefghij".repeat(6).into();
    let mut r = rows(1.0);
    r[1].label = long.clone();
    let bare = strip(1.0, r).render();
    // The name box is 34..118 without the pill: its ink reaches past where the pill will stand.
    assert!(bare.count_where(150 + 72, 0, 150 + 77, 35, |p| !ground(p)) > 0);
    let mut r = rows(1.0);
    r[1].label = long;
    decorate(&mut r[1]);
    let f = strip(1.0, r).render();
    // With it the box ends at 150 + 71 and the pill starts at 150 + 77: the gap between them is bare.
    assert_eq!(
        f.count_where(150 + 72, 0, 150 + 76, 35, |p| !ground(p)),
        0,
        "the name crossed into the pill"
    );
    assert!(
        f.count_where(150 + 34, 0, 150 + 71, 35, |p| !ground(p)) > 0,
        "the name is still drawn"
    );
}

#[test]
fn chips_hide_without_room_and_on_icon_tabs() {
    // No room: nothing.
    let mut r = rows(1.0);
    decorate(&mut r[1]);
    r[1].chip_room = 0;
    assert_eq!(ink_of_tab1(&strip(1.0, r).render(), 1.0), None);
    // Room for one: the progress only (8 + 18 wide).
    let mut r = rows(1.0);
    decorate(&mut r[1]);
    r[1].chip_room = 1;
    let (x0, _, x1, _) = ink_of_tab1(&strip(1.0, r).render(), 1.0).expect("one chip");
    assert!(
        (x1 + 1 - x0).abs_diff(26) <= 2,
        "one chip is {} wide",
        x1 + 1 - x0
    );
    // An icon tab never draws one, whatever its row says: only its centred icon has ink.
    let mut r = rows(1.0);
    r[1].mode = 1;
    r[1].w = 40;
    r[2].x = 190;
    decorate(&mut r[1]);
    let f = strip(1.0, r).render();
    assert_eq!(f.count_where(150, 0, 150 + 10, 35, |p| !ground(p)), 0);
    assert_eq!(f.count_where(150 + 30, 0, 150 + 38, 35, |p| !ground(p)), 0);
}

#[test]
fn a_chip_never_covers_the_agent_bar_at_the_tab_foot() {
    for s in SCALES {
        let mut r = rows(s);
        decorate(&mut r[1]);
        r[1].mark_kind = 2;
        r[1].mark_state = 1;
        r[1].mark_color = Color::from_rgb_u8(255, 0, 0);
        let f = strip(s, r).render();
        let foot = f.h - 1 - s.floor().max(1.0) as u32; // the strip is one hairline shorter than the window
        let bar = px(s, 3.0);
        // The pill's ink ends above the bar.
        let (x0, _, x1, y1) = f
            .bbox_where(px(s, 180.0), 0, px(s, 296.0), foot + 1 - bar, |p| {
                !ground(p) && !red(p)
            })
            .expect("the pill");
        assert!(y1 < foot + 1 - bar, "scale {s}: the pill reaches {y1}");
        // And under the pill, every pixel of the bar is the bar's colour.
        for x in x0..=x1 {
            for y in (foot + 1 - bar)..=foot {
                assert!(red(f.px(x, y)), "scale {s}: ({x}, {y}) is {:?}", f.px(x, y));
            }
        }
    }
}

#[test]
fn a_sidebar_row_draws_its_chips_and_only_that_row_changes() {
    let side = |decorated: bool| {
        let h = Harness::new(1.0, 700, 500, move || {
            let c = TabSidebarPreview::new().expect("the preview builds");
            let rows: Vec<TabRow> = (0..4)
                .map(|i| {
                    let mut r = TabRow {
                        id: format!("t{i}").into(),
                        label: "api".into(),
                        tip: "C:\\api".into(),
                        folder: "~\\api".into(),
                        kept: true,
                        prog: -1.0,
                        ..TabRow::default()
                    };
                    if decorated && i == 2 {
                        decorate(&mut r);
                        // The sidebar draws both whatever room the strip had.
                        r.chip_room = 0;
                    }
                    r
                })
                .collect();
            c.set_rows(ModelRc::new(VecModel::from(rows)));
            c
        });
        h.advance(0);
        h.render()
    };
    let (a, b) = (side(false), side(true));
    let mut changed: Option<(u32, u32, u32, u32)> = None;
    for y in 0..a.h {
        for x in 0..a.w {
            if !Frame::near(a.px(x, y), b.px(x, y), 3) {
                changed = Some(match changed {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    let (x0, y0, x1, y1) = changed.expect("the decorated row changed");
    // One pill with both chips (43 px) on one row (20 px), inside the 220 px sidebar.
    assert!((x1 + 1 - x0).abs_diff(43) <= 3, "{x0}..{x1}");
    assert!(y1 + 1 - y0 <= 22, "{y0}..{y1}");
    assert!(x1 < 220, "right of the sidebar: {x1}");
}

/// Every badge glyph draws at its full size (14 px, stroke 1.8) at every scaling: the box is grown by one stroke
/// and moved back by half, as every stroked glyph's is (`icons_tabs.rs`). The scan in `icons_misc.rs` lists it. The
/// pill's own outline is taken out first: the same pill with an empty glyph is drawn, and every pixel the two share
/// counts as ground.
#[test]
fn every_badge_glyph_draws_at_full_size_at_every_scale() {
    use super::icons_tabs::{Ink, expected_extent_px, ink_edges};
    const ACCENT_HI: [f32; 3] = [246.0, 162.0, 91.0];
    let one = |s: f32, cmd: &str| {
        let mut r = rows(s);
        r[1].chip_count = 1;
        r[1].chip1_cmd = cmd.into();
        r[1].chip1_tone = 1;
        r[1].chip_room = 2;
        strip(s, r).render()
    };
    for s in crate::ui_harness::SCALES {
        let outline = one(s, "");
        for icon in flare_ctl::decor::BADGE_ICONS {
            let cmd = glyph_path(crate::menus_ui::badge_glyph(icon).expect("a built-in badge"));
            let mut f = one(s, cmd);
            for (i, px) in f.rgba.chunks_mut(4).enumerate() {
                if px == &outline.rgba[i * 4..i * 4 + 4] {
                    px.copy_from_slice(&GROUND);
                }
            }
            let region = (px(s, 240.0), 0, px(s, 280.0), f.h - 1);
            let got = ink_edges(&f, region, &Ink::new(GROUND, ACCENT_HI))
                .unwrap_or_else(|| panic!("{icon} at {s} is drawn"));
            let want = expected_extent_px(cmd, 14.0, 24.0, 1.8 * 14.0 / 24.0, s);
            let (w, h) = (got.2 - got.0, got.3 - got.1);
            assert!(
                (w - want.0).abs() <= 1.0,
                "{icon} at {s}: width {w}, want {}",
                want.0
            );
            assert!(
                (h - want.1).abs() <= 1.0,
                "{icon} at {s}: height {h}, want {}",
                want.1
            );
        }
    }
}
