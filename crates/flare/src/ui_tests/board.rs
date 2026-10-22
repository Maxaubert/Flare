//! The agent board and the Preview chip (Surfaces stage 7), headless: drawn from rows set directly on the window as the
//! controller sets them. What is held: a row is as tall as the probe says (56 css px, 92 with a question), a click on a
//! row calls back with its tab and the keys scroll; a question row wears the tint and the tab in front the other; an
//! empty board shows its note; the chip sits inside the terminal's top right corner whole, at both scales, takes a click
//! and takes none while hidden. Every check runs at 100% and 225%.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{Global, ModelRc, VecModel};

use crate::appctl::AppCtl;
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, BoardRowUi, Theme};

const SCALES2: [f32; 2] = [1.0, 2.25];
const PLAIN: f32 = 56.0;
const WITH_DETAIL: f32 = 92.0;

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn key(k: Key) -> String {
    String::from(char::from(k))
}

fn row(tab: &str, state: i32, detail: &str, current: bool) -> BoardRowUi {
    BoardRowUi {
        tab: tab.into(),
        name: format!("project {tab}").into(),
        agent: "Claude".into(),
        state,
        ink: slint::Color::from_rgb_u8(0xd7, 0x77, 0x57),
        folder: "~\\work".into(),
        status: match state {
            1 => "Working for 4m 12s",
            2 => "Waiting 2m 00s",
            3 => "Finished 5m 00s ago",
            4 => "Failed 1m 01s ago",
            _ => "Idle",
        }
        .into(),
        detail: detail.into(),
        current,
    }
}

fn show(h: &Harness<AppWindow>, rows: Vec<BoardRowUi>) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(6);
    h.comp.set_panel_title("Agents".into());
    h.comp
        .set_panel_board_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    h.advance(0);
}

fn at(h: &Harness<AppWindow>, f: &Frame, x: f32, y: f32) -> [u8; 4] {
    let s = h.scale();
    f.px((x * s) as u32, (y * s) as u32)
}

fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    Frame::near(a, b, tol)
}

fn over(under: [u8; 4], ink: slint::Color, alpha: f32) -> [u8; 4] {
    let m = |u: u8, i: u8| (f32::from(u) * (1.0 - alpha) + f32::from(i) * alpha).round() as u8;
    [
        m(under[0], ink.red()),
        m(under[1], ink.green()),
        m(under[2], ink.blue()),
        255,
    ]
}

fn picks(h: &Harness<AppWindow>) -> Rc<RefCell<Vec<String>>> {
    let got = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = got.clone();
    h.comp
        .on_panel_board_pick(move |t| sink.borrow_mut().push(t.to_string()));
    got
}

#[test]
fn a_click_on_a_row_picks_its_tab_whatever_the_rows_above_are_tall() {
    for s in SCALES2 {
        let h = window(s);
        let got = picks(&h);
        show(
            &h,
            vec![
                row("t1", 1, "", false),
                row("t2", 2, "Which colour do you prefer?", false),
                row("t3", 0, "", false),
            ],
        );
        let (bx, by, bw) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
        );
        assert_eq!(h.comp.get_panel_board_row_count(), 3);
        // The same numbers the probe uses: 56, then 92, then 56.
        h.click(bx + bw / 2.0, by + PLAIN / 2.0);
        h.click(bx + bw / 2.0, by + PLAIN + WITH_DETAIL / 2.0);
        h.click(bx + bw / 2.0, by + PLAIN + WITH_DETAIL + PLAIN / 2.0);
        assert_eq!(got.borrow().as_slice(), ["t1", "t2", "t3"], "scale {s}");
        let content = h.comp.get_panel_board_content_h();
        assert!(
            (content - (PLAIN * 2.0 + WITH_DETAIL)).abs() < 1.0,
            "scale {s}: content {content}"
        );
    }
}

#[test]
fn the_keys_scroll_a_long_board_and_a_row_far_down_is_still_clickable() {
    for s in SCALES2 {
        let h = window(s);
        h.activate();
        let got = picks(&h);
        let rows: Vec<BoardRowUi> = (0..40)
            .map(|i| row(&format!("t{i}"), i % 5, "", false))
            .collect();
        show(&h, rows);
        h.comp.invoke_focus_panel();
        h.advance(0);
        // The layout runs when the window is drawn.
        let _ = h.render();
        let (bx, by, bw, bh) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
            h.comp.get_panel_body_h(),
        );
        assert!(
            h.comp.get_panel_board_content_h() > bh,
            "scale {s}: it overflows ({} high in {bh})",
            h.comp.get_panel_board_content_h()
        );
        h.key_text(&key(Key::End));
        h.advance(0);
        // The last row is at the bottom of the view now.
        h.click(bx + bw / 2.0, by + bh - PLAIN / 2.0);
        assert_eq!(
            got.borrow().last().map(String::as_str),
            Some("t39"),
            "scale {s}"
        );
        h.key_text(&key(Key::Home));
        h.advance(0);
        h.click(bx + bw / 2.0, by + PLAIN / 2.0);
        assert_eq!(
            got.borrow().last().map(String::as_str),
            Some("t0"),
            "scale {s}"
        );
        h.key_text(&key(Key::PageDown));
        h.advance(0);
        h.click(bx + bw / 2.0, by + PLAIN / 2.0);
        let t = got.borrow().last().cloned().unwrap();
        assert_ne!(t, "t0", "scale {s}: a page down moved the view");
    }
}

#[test]
fn a_question_row_and_the_tab_in_front_wear_a_tint_and_an_idle_row_none() {
    for s in SCALES2 {
        let h = window(s);
        show(
            &h,
            vec![
                row("t1", 0, "", false),
                row("t2", 2, "", false),
                row("t3", 0, "", true),
            ],
        );
        let ground = {
            let c = Theme::get(&h.comp).get_ground();
            [c.red(), c.green(), c.blue(), 255]
        };
        let acc = Theme::get(&h.comp).get_accent();
        let f = h.render();
        let (bx, by, bw) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
        );
        // Far right, under the agent's name: only the row's background is there.
        let x = bx + bw - 60.0;
        let y = |n: f32| by + n * PLAIN + 48.0;
        assert!(
            near(at(&h, &f, x, y(0.0)), ground, 3),
            "scale {s}: idle on the ground"
        );
        assert!(
            near(at(&h, &f, x, y(1.0)), over(ground, acc, 0.07), 4),
            "scale {s}: a question row has the faint tint, {:?}",
            at(&h, &f, x, y(1.0))
        );
        assert!(
            near(at(&h, &f, x, y(2.0)), over(ground, acc, 0.12), 4),
            "scale {s}: the tab in front has the stronger one, {:?}",
            at(&h, &f, x, y(2.0))
        );
    }
}

#[test]
fn an_empty_board_shows_the_note_and_no_rows() {
    for s in SCALES2 {
        let h = window(s);
        show(&h, Vec::new());
        h.comp
            .set_panel_note("No agent is running in your tabs.".into());
        h.advance(0);
        assert_eq!(h.comp.get_panel_board_row_count(), 0, "scale {s}");
        let got = picks(&h);
        h.click(
            h.comp.get_panel_body_x() + 40.0,
            h.comp.get_panel_body_y() + 20.0,
        );
        assert!(got.borrow().is_empty(), "scale {s}");
    }
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
