//! The plugins' session bar (plugins phase 6), headless: the terminal gives up the bar's height and the bar row sits
//! under it, as wide as the grid, at every scale; the page's place leaves the chips their room on the right; a chip
//! click says which; and nothing of it shows while the grid is not in front.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::menus_ui::{GLYPH_BADGE_BOLT, glyph_path};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, PluginButton};

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.comp.set_has_tabs(true);
    h.comp.set_front_kind(0);
    h.advance(0);
    h
}

fn chips(n: usize) -> ModelRc<PluginButton> {
    let v: Vec<PluginButton> = (0..n)
        .map(|i| PluginButton {
            cmd: if i == 0 {
                glyph_path(GLYPH_BADGE_BOLT).into()
            } else {
                "".into()
            },
            letter: "S".into(),
            tip: format!("Plugin player: chip {i}").into(),
            checked: false,
            enabled: true,
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

#[test]
fn the_terminal_gives_up_the_bars_height_and_the_bar_sits_under_it_as_wide_as_the_grid() {
    for s in SCALES {
        let h = window(s);
        let resized = Rc::new(RefCell::new(0u32));
        {
            let r = resized.clone();
            h.comp.on_grid_resized(move |_, _, _| *r.borrow_mut() += 1);
        }
        let (gh0, top) = (h.comp.get_grid_phys_height(), h.comp.get_grid_top());
        assert!(!h.comp.get_bar_shown());
        h.comp.set_bar_h(48.0);
        h.advance(0);
        assert!(h.comp.get_bar_shown());
        let gh1 = h.comp.get_grid_phys_height();
        assert!(
            (gh0 - gh1 - (48.0 * s).round() as i32).abs() <= 1,
            "{s}: {gh0} -> {gh1}"
        );
        assert_eq!(*resized.borrow(), 1, "{s}: one fit, no flicker of sizes");
        // The page's place: right under the grid and its top line, as wide as the grid with no chips.
        let grid_bottom = top + gh1 as f32 / s;
        assert!(
            (h.comp.get_bar_web_y() - grid_bottom).abs() <= 1.5,
            "{s}: bar at {}, grid ends at {grid_bottom}",
            h.comp.get_bar_web_y()
        );
        let gw = h.comp.get_grid_phys_width() as f32 / s;
        assert!((h.comp.get_bar_web_w() - gw).abs() <= 1.0, "{s}");
        assert!((h.comp.get_bar_web_h() - 47.0).abs() <= 1.0, "{s}");
        // Two chips take their room at the right end, and the page's place gives it up.
        h.comp.set_bar_chips(chips(2));
        h.advance(0);
        assert!((h.comp.get_bar_web_w() - (gw - 66.0)).abs() <= 1.0, "{s}");
        assert!(h.comp.get_bar_chips_x() >= h.comp.get_bar_web_x() + h.comp.get_bar_web_w() - 0.5);
        // The grid is unchanged by the chips.
        assert_eq!(h.comp.get_grid_phys_height(), gh1);
        // Gone: the terminal has its height back.
        h.comp.set_bar_h(0.0);
        h.advance(0);
        assert_eq!(h.comp.get_grid_phys_height(), gh0, "{s}");
        assert!(!h.comp.get_bar_shown());
    }
}

#[test]
fn a_chip_click_says_which_and_the_chips_are_drawn_in_the_bar_row() {
    let h = window(1.0);
    let bare = {
        let b = window(1.0);
        b.comp.set_bar_h(40.0);
        b.advance(0);
        b.render()
    };
    h.comp.set_bar_h(40.0);
    h.comp.set_bar_chips(chips(2));
    h.advance(0);
    let clicked = Rc::new(RefCell::new(Vec::new()));
    {
        let c = clicked.clone();
        h.comp.on_bar_chip_clicked(move |i| c.borrow_mut().push(i));
    }
    let (x, y) = (h.comp.get_bar_chips_x(), h.comp.get_bar_web_y());
    let mid = y + 20.0;
    h.click(x + 6.0 + 13.0, mid);
    h.click(x + 6.0 + 30.0 + 13.0, mid);
    assert_eq!(*clicked.borrow(), [0, 1]);
    // Ink only inside the chips' room of the row.
    let f = h.render();
    let mut ink = (u32::MAX, 0u32);
    for py in (y as u32)..((y + 39.0) as u32) {
        for px in 0..1000u32 {
            if !Frame::near(bare.px(px, py), f.px(px, py), 3) {
                ink = (ink.0.min(px), ink.1.max(px));
            }
        }
    }
    assert!(
        ink.0 >= x as u32,
        "chip ink starts at {} before the chips' room at {x}",
        ink.0
    );
    assert!(ink.1 > ink.0, "the chips are drawn");
}

#[test]
fn nothing_of_the_bar_shows_while_the_grid_is_not_in_front() {
    let h = window(1.0);
    let gh0 = h.comp.get_grid_phys_height();
    h.comp.set_front_kind(1);
    h.comp.set_bar_h(48.0);
    h.advance(0);
    assert!(!h.comp.get_bar_shown());
    h.comp.set_front_kind(0);
    h.advance(0);
    assert!(h.comp.get_bar_shown());
    assert!(h.comp.get_grid_phys_height() < gh0);
}
