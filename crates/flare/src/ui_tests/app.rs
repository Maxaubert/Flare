//! Component tests for the assembled window (Task 25), headless: sample properties set directly,
//! no Rust glue. Pixel facts are measured at every display scaling.

use std::cell::Cell;
use std::rc::Rc;

use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, Place, StripGeom, TabRow};

const GROUND: [u8; 4] = [23, 23, 23, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];
const SECONDARY: [u8; 4] = [13, 13, 13, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];

fn is_red(p: [u8; 4]) -> bool {
    Frame::near(p, RED, 2)
}

fn bright(p: [u8; 4]) -> bool {
    (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 90
}

fn tab_rows(scale: f32, n: usize) -> Vec<TabRow> {
    (0..n)
        .map(|i| TabRow {
            id: format!("t{i}").into(),
            label: format!("tab {i}").into(),
            active: i == 0,
            kept: true,
            mode: 0,
            x: (i as f32 * 150.0 * scale).round() as i32,
            w: (150.0 * scale).round() as i32,
            ..TabRow::default()
        })
        .collect()
}

fn red_image() -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(8, 8);
    for p in buf.make_mut_slice() {
        *p = Rgba8Pixel {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
    }
    Image::from_rgba8(buf)
}

/// A 1000 x 640 window with three tabs and a red grid image.
fn harness(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.comp
        .set_tab_rows(ModelRc::new(VecModel::from(tab_rows(scale, 3))));
    h.comp.set_has_tabs(true);
    h.comp.set_strip_geom(StripGeom {
        plus_x: (450.0 * scale).round() as i32,
        plus_w: (32.0 * scale).round() as i32,
        ..StripGeom::default()
    });
    h.comp.set_grid_image(red_image());
    h.advance(0);
    h
}

fn hair(scale: f32) -> u32 {
    scale.floor().max(1.0) as u32
}

#[test]
fn the_window_lays_out_at_every_scale_without_a_stray_pixel_row() {
    for s in SCALES {
        let h = harness(s);
        let f = h.render();
        assert!(
            h.comp.get_grid_phys_width() * h.comp.get_grid_phys_height() > 0,
            "scale {s}"
        );
        let foot = h.phys(36.0);
        let hr = hair(s);
        let x = h.phys(700.0);
        // The divider rows are exactly the top row's foot, and every one is solid across.
        let divider_rows: Vec<u32> = (0..foot + 4)
            .filter(|y| Frame::near(f.px(x, *y), DIVIDER, 2))
            .collect();
        let expect: Vec<u32> = (foot - hr..foot).collect();
        assert_eq!(divider_rows, expect, "scale {s}");
        for y in foot - hr..foot {
            let run = f.run_len_h(y, h.phys(520.0), |p| Frame::near(p, DIVIDER, 2));
            assert!(run >= h.phys(300.0), "scale {s} row {y} run {run}");
        }
        // The row under the hairline is the page.
        assert!(is_red(f.px(x, foot)), "scale {s}");
    }
}

#[test]
fn the_top_row_is_36_css_px_and_the_page_starts_under_it() {
    for s in [1.0, 1.5, 2.25] {
        let h = harness(s);
        let f = h.render();
        let first = f.bbox_where(0, 0, f.w, f.h, is_red).expect("page pixels");
        assert_eq!(first.1, (36.0 * s).round() as u32, "scale {s}");
    }
}

#[test]
fn vertical_layout_has_a_bar_beside_the_sidebar_and_no_strip() {
    let h = harness(1.0);
    h.comp.set_layout_vertical(true);
    h.comp.set_sidebar_width(220.0);
    h.advance(0);
    let f = h.render();
    assert!(
        Frame::near(f.px(500, 5), SECONDARY, 2),
        "bar bg {:?}",
        f.px(500, 5)
    );
    assert!(Frame::near(f.px(300, 30), SECONDARY, 2));
    // No strip: no tab label ink in the bar between the mark and the buttons.
    assert_eq!(f.count_where(60, 0, 800, 36, bright), 0);
    // The sidebar's rows start under the bar and are there.
    assert_eq!(f.count_where(40, 0, 220, 36, bright), 0);
    assert!(f.count_where(0, 36, 220, 300, bright) > 0);
}

#[test]
fn the_open_sidebar_cuts_the_terminals_corner_and_draws_its_edge() {
    for s in SCALES {
        let h = harness(s);
        h.comp.set_layout_vertical(true);
        h.comp.set_sidebar_width(220.0);
        h.advance(0);
        let f = h.render();
        let px = |x: f32, y: f32| f.px(h.phys(x), h.phys(y));
        // Outside the arc: the sidebar's colour. Inside: the page.
        let out_px = f.px(h.phys(220.0) + hair(s) + 1, h.phys(36.0) + hair(s) + 1);
        assert!(
            Frame::near(out_px, SECONDARY, 2),
            "scale {s} outside {:?}",
            out_px
        );
        assert!(
            is_red(px(229.0, 45.0)),
            "scale {s} inside {:?}",
            px(229.0, 45.0)
        );
        // The divider runs down at x = 220 css.
        let x = h.phys(220.0);
        let y = h.phys(436.0);
        // The divider is translucent: here it lies over the red page.
        assert!(
            Frame::near(f.px(x, y), [255, 18, 18, 255], 2),
            "scale {s} line {:?}",
            f.px(x, y)
        );
        assert!(
            Frame::near(f.px(x - 1, y), SECONDARY, 2),
            "scale {s} left of line"
        );
        assert!(is_red(f.px(x + hair(s), y)), "scale {s} right of line");
    }
}

#[test]
fn a_collapsed_sidebar_takes_no_room_and_slides_over_the_page() {
    let h = harness(1.0);
    h.comp.set_layout_vertical(true);
    h.comp.set_sidebar_width(220.0);
    h.comp.set_sidebar_collapsed(true);
    h.advance(0);
    let f = h.render();
    assert!(is_red(f.px(2, 150)), "the page starts at x 0");
    assert_eq!(f.count_where(0, 80, 200, 300, bright), 0);
    h.comp.set_sidebar_revealed(true);
    for _ in 0..16 {
        h.advance(16);
    }
    h.advance(240);
    let f = h.render();
    assert!(!is_red(f.px(100, 150)), "the sidebar is over the page");
    assert!(f.count_where(0, 80, 200, 300, bright) > 0);
    assert!(is_red(f.px(500, 150)));
}

#[test]
fn the_grid_keeps_its_width_while_frozen() {
    let h = harness(1.0);
    h.comp.set_layout_vertical(true);
    h.comp.set_sidebar_width(220.0);
    h.comp.set_sidebar_collapsed(true);
    h.comp.set_grid_frozen_width(500.0);
    h.advance(0);
    let counter = Rc::new(Cell::new(0));
    {
        let c = counter.clone();
        h.comp.on_grid_resized(move |_, _, _| c.set(c.get() + 1));
    }
    h.advance(0);
    let _ = h.render();
    counter.set(0);
    let before = h.comp.get_grid_phys_width();
    assert_eq!(before, 500);
    // Pin the sidebar: the slot animates, the page column slides right, the grid image keeps its width.
    h.comp.set_sidebar_sliding(true);
    h.comp.set_sidebar_collapsed(false);
    for _ in 0..15 {
        h.advance(16);
        let f = h.render();
        let (x0, _, x1, _) = f
            .bbox_where(0, 100, f.w, 400, is_red)
            .expect("the image is on screen");
        let w = x1 - x0 + 1;
        assert!(
            (499..=501).contains(&w),
            "frozen width during the slide: {w}"
        );
    }
    h.advance(40);
    let f = h.render();
    let (x0, _, x1, _) = f.bbox_where(0, 100, f.w, 400, is_red).unwrap();
    assert!(x0 == 220 || x0 == 221, "left edge {x0}");
    assert_eq!(x1, 719);
    assert_eq!(counter.get(), 0, "no resize during the slide");
    assert_eq!(h.comp.get_grid_phys_width(), 500);
    h.comp.set_grid_frozen_width(0.0);
    h.advance(0);
    let _ = h.render();
    assert_eq!(counter.get(), 1, "one resize after the thaw");
    assert_eq!(h.comp.get_grid_phys_width(), 780);
}

#[test]
fn front_kind_selects_the_page() {
    let h = harness(1.0);
    let page = |f: &Frame| (0, 40, f.w, f.h);
    // 0: the grid.
    let f = h.render();
    assert!(f.count_where(0, 40, f.w, f.h, is_red) > 0);
    // 1: the word Settings and no grid.
    h.comp.set_front_kind(1);
    h.advance(0);
    let f = h.render();
    let (a, b, c, d) = page(&f);
    assert_eq!(f.count_where(a, b, c, d, is_red), 0);
    assert!(f.count_where(a, b, c, d, bright) > 0, "the word Settings");
    // 2: the start screen's mark in the accent.
    h.comp.set_front_kind(2);
    h.advance(0);
    h.advance(450);
    let f = h.render();
    assert_eq!(f.count_where(0, 40, f.w, f.h, is_red), 0);
    let accent = |p: [u8; 4]| Frame::near(p, ACCENT, 12);
    assert!(f.count_where(0, 40, f.w, f.h, accent) > 0, "the mark");
    // 3: bare ground.
    h.comp.set_front_kind(3);
    h.advance(0);
    let f = h.render();
    let ground = |p: [u8; 4]| Frame::near(p, GROUND, 1);
    let n = f.count_where(0, 40, f.w, f.h, ground);
    assert_eq!(n, ((f.h - 40) * f.w) as usize);
}

#[test]
fn popups_are_pointer_transparent_while_closed() {
    let h = harness(1.0);
    h.comp.set_front_kind(2);
    h.comp
        .set_places(ModelRc::new(VecModel::from(Vec::<Place>::new())));
    h.advance(0);
    h.advance(450);
    let hits = Rc::new(Cell::new(0));
    {
        let c = hits.clone();
        h.comp.on_start_new(move || c.set(c.get() + 1));
    }
    // The page is 604 high with a 36 footer: the block (261) is centred above it, New is its 4th piece.
    let new_y = 36.0 + ((604.0_f32 - 36.0 - 261.0) / 2.0).round() + 84.0 + 16.0 + 28.0 + 40.0;
    h.click(500.0, new_y + 18.0);
    assert_eq!(hits.get(), 1);
}

#[test]
fn chrome_has_focus_follows_the_find_box_and_the_dialog() {
    let h = harness(1.0);
    h.comp.set_layout_vertical(true);
    h.advance(0);
    assert!(!h.comp.get_chrome_has_focus());
    h.comp.invoke_focus_sidebar_find();
    assert!(h.comp.get_chrome_has_focus());
    h.comp.invoke_focus_terminal();
    assert!(!h.comp.get_chrome_has_focus());
    h.comp.set_dialog_open(true);
    assert!(h.comp.get_chrome_has_focus());
}

#[test]
fn the_title_buttons_are_unchanged() {
    let h = harness(1.0);
    let n = Rc::new(Cell::new([0u32; 3]));
    {
        let c = n.clone();
        h.comp.on_minimize_clicked(move || {
            let mut v = c.get();
            v[0] += 1;
            c.set(v);
        });
        let c = n.clone();
        h.comp.on_maximize_clicked(move || {
            let mut v = c.get();
            v[1] += 1;
            c.set(v);
        });
        let c = n.clone();
        h.comp.on_close_clicked(move || {
            let mut v = c.get();
            v[2] += 1;
            c.set(v);
        });
    }
    h.click(906.0, 18.0);
    h.click(942.0, 18.0);
    h.click(978.0, 18.0);
    assert_eq!(n.get(), [1, 1, 1]);
    h.move_to(978.0, 18.0);
    let f = h.render();
    // Tailwind red-500 at 80% over the ground.
    let p = f.px(966, 8);
    assert!(Frame::near(p, [205, 40, 48, 255], 3), "close hover {p:?}");
}

#[test]
fn double_click_on_the_empty_strip_maximises() {
    let h = harness(1.0);
    let n = Rc::new(Cell::new(0));
    {
        let c = n.clone();
        h.comp.on_bar_double_clicked(move || c.set(c.get() + 1));
    }
    h.click(800.0, 18.0);
    h.click(800.0, 18.0);
    assert_eq!(n.get(), 1);
}
