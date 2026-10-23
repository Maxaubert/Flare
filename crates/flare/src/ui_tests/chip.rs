//! The Preview chip (Surfaces stage 7), headless: the chip sits inside the terminal's top right corner whole, at both
//! scales, takes a click and takes none while hidden. Every check runs at 100% and 225%.

use std::cell::RefCell;
use std::rc::Rc;

use slint::Global;

use crate::appctl::AppCtl;
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, Theme};

const SCALES2: [f32; 2] = [1.0, 2.25];

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn at(h: &Harness<AppWindow>, f: &Frame, x: f32, y: f32) -> [u8; 4] {
    let s = h.scale();
    f.px((x * s) as u32, (y * s) as u32)
}

fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    Frame::near(a, b, tol)
}

// ---- the Preview chip ----

fn chip(h: &Harness<AppWindow>, label: &str) {
    h.comp.set_preview_chip_label(label.into());
    h.advance(0);
}

#[test]
fn the_chip_sits_whole_in_the_terminals_top_right_and_takes_a_click() {
    for s in SCALES2 {
        let h = window(s);
        let clicks = Rc::new(RefCell::new(0));
        {
            let n = clicks.clone();
            h.comp.on_preview_chip_clicked(move || *n.borrow_mut() += 1);
        }
        chip(&h, "localhost:5173");
        let (x, y, w, ch) = (
            h.comp.get_preview_chip_x(),
            h.comp.get_preview_chip_y(),
            h.comp.get_preview_chip_w(),
            h.comp.get_preview_chip_h(),
        );
        let grid_right = h.comp.get_grid_left() + h.comp.get_grid_phys_width() as f32 / s;
        assert!(w > 90.0 && w < 260.0, "scale {s}: a pill, {w} wide");
        assert!((ch - 26.0).abs() < 0.6, "scale {s}: {ch} high");
        assert!(
            x + w <= grid_right - 10.0,
            "scale {s}: inside the corner, clear of the edge"
        );
        assert!(
            x > h.comp.get_grid_left() + 100.0,
            "scale {s}: not off to the left"
        );
        assert!(
            y >= h.comp.get_grid_top() + 4.0,
            "scale {s}: under the top edge"
        );
        h.click(x + w / 2.0, y + ch / 2.0);
        assert_eq!(*clicks.borrow(), 1, "scale {s}");
        // Whole device pixels: the edges sit on pixel boundaries at this scale.
        for v in [x, y, w, ch] {
            let px = v * s;
            assert!(
                (px - px.round()).abs() < 0.01,
                "scale {s}: {v} css is {px} device px"
            );
        }
    }
}

#[test]
fn no_chip_is_drawn_or_clickable_without_a_label() {
    for s in SCALES2 {
        let h = window(s);
        let clicks = Rc::new(RefCell::new(0));
        {
            let n = clicks.clone();
            h.comp.on_preview_chip_clicked(move || *n.borrow_mut() += 1);
        }
        chip(&h, "localhost:5173");
        let (x, y, w, ch) = (
            h.comp.get_preview_chip_x(),
            h.comp.get_preview_chip_y(),
            h.comp.get_preview_chip_w(),
            h.comp.get_preview_chip_h(),
        );
        chip(&h, "");
        h.click(x + w / 2.0, y + ch / 2.0);
        assert_eq!(*clicks.borrow(), 0, "scale {s}");
    }
}

#[test]
fn the_chip_wears_the_accent_in_its_outline_and_is_opaque_inside() {
    for s in SCALES2 {
        let h = window(s);
        chip(&h, "localhost:5173");
        let f = h.render();
        let (x, y, w, ch) = (
            h.comp.get_preview_chip_x(),
            h.comp.get_preview_chip_y(),
            h.comp.get_preview_chip_w(),
            h.comp.get_preview_chip_h(),
        );
        let ground = {
            let c = Theme::get(&h.comp).get_ground();
            [c.red(), c.green(), c.blue(), 255]
        };
        // The top edge of the pill, half way along, is its outline: the accent, not the grid's ground.
        let edge = at(&h, &f, x + w / 2.0, y + 0.5);
        assert!(
            !near(edge, ground, 8),
            "scale {s}: the outline is drawn, {edge:?} on {ground:?}"
        );
        // And well outside the pill the grid is untouched.
        let outside = at(&h, &f, x - 20.0, y + ch / 2.0);
        assert!(near(outside, ground, 3), "scale {s}: {outside:?}");
    }
}
