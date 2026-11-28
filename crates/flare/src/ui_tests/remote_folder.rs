//! The remote folder browser (rule 1282, Max's pick of mockup B on 2026-11-17), headless: the Dialog look at every scale
//! in a dark and a light look (the box, the list's ground, a picked row's accent wash, a folder's colour, the accent
//! Open here, the one error line under the list), and what the pointer and the keys reach (a row, a crumb, Back and
//! Home with their tips, Cancel and a press outside, Escape, Enter and the arrows) with nothing behind it reached.

use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::icons_tabs::blend;
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{RemoteCrumb, RemoteFolderPreview, RemoteFolderRow};

fn rgb(c: slint::Color) -> [f32; 3] {
    [
        f32::from(c.red()),
        f32::from(c.green()),
        f32::from(c.blue()),
    ]
}

fn near(p: [u8; 4], w: [f32; 3], tol: f32) -> bool {
    (0..3).all(|c| (f32::from(p[c]) - w[c]).abs() <= tol)
}

/// A look's chrome tokens written into the preview's Theme (the ones the dialog draws with).
fn paint(h: &Harness<RemoteFolderPreview>, id: &str) {
    use crate::chrome_apply::colour;
    use flare_theme::edges::DEFAULT_WINDOW_EDGES;
    use flare_theme::look::{LookInput, apply_look};
    let l = apply_look(&LookInput {
        theme_id: id,
        custom: None,
        draft: None,
        background: None,
        accent: None,
        secondary: None,
        edges: DEFAULT_WINDOW_EDGES,
    });
    let tok = |n: &str| colour(&l.chrome.vars[n]);
    let t = h.comp.global::<crate::Theme>();
    t.set_light(l.light);
    t.set_ground(tok("--p-bg"));
    t.set_side_flat(tok("--p-side-flat"));
    t.set_text(tok("--p-text"));
    t.set_text_soft(tok("--p-text-soft"));
    t.set_dim(tok("--p-dim"));
    t.set_dim2(tok("--p-dim2"));
    t.set_hover(tok("--p-hover"));
    t.set_divider(tok("--p-divider"));
    t.set_line(tok("--p-line"));
    t.set_accent(tok("--p-accent"));
    t.set_accent_hi(tok("--p-accent-hi"));
    t.set_on_accent(tok("--p-on-accent"));
    t.set_tree_folder(tok("--p-tree-folder"));
    h.advance(0);
}

fn rows(list: &[(&str, bool)]) -> ModelRc<RemoteFolderRow> {
    ModelRc::new(Rc::new(VecModel::from(
        list.iter()
            .map(|(n, d)| RemoteFolderRow {
                name: (*n).into(),
                dir: *d,
            })
            .collect::<Vec<_>>(),
    )))
}

fn crumbs(list: &[&str]) -> ModelRc<RemoteCrumb> {
    let n = list.len();
    ModelRc::new(Rc::new(VecModel::from(
        list.iter()
            .enumerate()
            .map(|(i, l)| RemoteCrumb {
                label: (*l).into(),
                live: i + 1 < n,
                last: i + 1 == n,
            })
            .collect::<Vec<_>>(),
    )))
}

/// The mockup's dialog: home-pc's home, Documents picked.
fn harness(s: f32) -> Harness<RemoteFolderPreview> {
    let h = Harness::new(s, 1000, 640, || RemoteFolderPreview::new().unwrap());
    h.comp
        .set_crumbs(crumbs(&["home-pc", "C:", "Users", "Admin"]));
    h.comp.set_rows(rows(&[
        (".claude", true),
        ("Desktop", true),
        ("Documents", true),
        ("Downloads", true),
        ("Music", true),
        (".gitconfig", false),
        ("notes.txt", false),
    ]));
    h.comp.set_selected(2);
    h.comp.set_path(r"C:\Users\Admin\Documents".into());
    h
}

fn open(h: &Harness<RemoteFolderPreview>) -> Frame {
    h.comp.set_open(true);
    h.advance(0);
    h.render();
    h.advance(0);
    h.render()
}

/// Device pixel of a css point.
fn dev(s: f32, x: f32, y: f32) -> (u32, u32) {
    ((x * s) as u32, (y * s) as u32)
}

/// Rule 1282: the Dialog look at every scale in both looks: a 430 px side-flat box in the middle, the list on the
/// ground, a picked row washed in the accent, a folder in the folder colour, Open here in the accent, and the error as
/// one line under the list, inside the box.
#[test]
fn the_folder_browser_wears_the_dialog_look_at_every_scale_in_both_looks() {
    for look in ["pt-default", "paper"] {
        for s in SCALES {
            let what = format!("{look} at {s}");
            let h = harness(s);
            paint(&h, look);
            let f = open(&h);
            let theme = h.comp.global::<crate::Theme>();
            let (bx, by, bw, bh) = (
                h.comp.get_box_x(),
                h.comp.get_box_y(),
                h.comp.get_box_w(),
                h.comp.get_box_h(),
            );
            assert!((bw - 430.0).abs() < 0.5, "{what}: the box is 430 px ({bw})");
            assert!(
                (bx + bw / 2.0 - 500.0).abs() <= 1.0 && (by + bh / 2.0 - 320.0).abs() <= 1.0,
                "{what}: the box is in the middle"
            );
            // The box: side-flat, at its padding.
            let (x, y) = dev(s, bx + bw - 8.0, by + 8.0);
            assert!(
                near(f.px(x, y), rgb(theme.get_side_flat()), 3.0),
                "{what}: the box is side-flat ({:?})",
                f.px(x, y)
            );
            let (lx, ly, lw, lh) = (
                h.comp.get_list_x(),
                h.comp.get_list_y(),
                h.comp.get_list_w(),
                h.comp.get_list_h(),
            );
            assert!(lx > bx && lx + lw < bx + bw && ly > by && ly + lh < by + bh);
            // The list's ground, under its last row.
            let (x, y) = dev(s, lx + lw / 2.0, ly + lh - 10.0);
            let ground = f.px(x, y);
            assert!(
                near(ground, rgb(theme.get_ground()), 3.0),
                "{what}: the list is on the ground ({ground:?})"
            );
            let hair = s.floor().max(1.0) / s;
            let row_top = |i: f32| ly + hair + i * 28.0;
            // The picked row (2): the accent at 15 percent, above its words.
            let (x, y) = dev(s, lx + lw * 0.55, row_top(2.0) + 2.0);
            let want = blend(ground, rgb(theme.get_accent()), 0.15);
            assert!(
                near(f.px(x, y), want, 6.0),
                "{what}: the picked row's wash ({:?}, want {want:?})",
                f.px(x, y)
            );
            // Row 0's folder: the folder colour at 0.85, in the glyph's solid body (units 12, 13 of its 14 px box).
            let (x, y) = dev(
                s,
                lx + hair + 10.0 + 12.0 * 14.0 / 24.0,
                row_top(0.0) + 7.0 + 13.0 * 14.0 / 24.0,
            );
            let want = blend(ground, rgb(theme.get_tree_folder()), 0.85);
            assert!(
                near(f.px(x, y), want, 14.0),
                "{what}: a folder wears the folder colour ({:?}, want {want:?})",
                f.px(x, y)
            );
            // Open here: the accent, left of its words; Cancel is the plain button beside it.
            let (x, y) = dev(
                s,
                h.comp.get_open_x() + 6.0,
                h.comp.get_open_y() + h.comp.get_open_h() / 2.0,
            );
            assert!(
                near(f.px(x, y), rgb(theme.get_accent()), 4.0),
                "{what}: Open here is the accent ({:?})",
                f.px(x, y)
            );
            assert!(h.comp.get_cancel_x() + h.comp.get_cancel_w() <= h.comp.get_open_x());
            assert!(h.comp.get_open_x() + h.comp.get_open_w() <= bx + bw - 19.0);
            assert!(!h.comp.get_error_shown());
            // The error: one line under the list, above the buttons, in the error ink.
            h.comp.set_error("Not answering".into());
            h.advance(0);
            h.render();
            let f = h.render();
            assert!(h.comp.get_error_shown());
            // The box grew and stays in the middle: the list moved with it.
            let (lx, ly, lh) = (
                h.comp.get_list_x(),
                h.comp.get_list_y(),
                h.comp.get_list_h(),
            );
            let ey = h.comp.get_error_y();
            assert!(
                ey >= ly + lh && ey + 26.0 <= h.comp.get_open_y(),
                "{what}: the line sits between the list and the buttons"
            );
            let err = if theme.get_light() {
                [0xc4_u8, 0x28, 0x2e]
            } else {
                [0xff_u8, 0x8a, 0x8d]
            };
            let ink = f.count_where(
                (lx * s) as u32,
                (ey * s) as u32,
                ((lx + 200.0) * s) as u32,
                ((ey + 26.0) * s) as u32,
                |p| near(p, err.map(f32::from), 40.0),
            );
            assert!(ink > 0, "{what}: the line is drawn in the error ink");
            assert!(
                h.comp.get_box_h() > bh,
                "{what}: the line takes its own room in the box"
            );
        }
    }
}

fn centre(x: f32, y: f32, w: f32, hh: f32) -> (f32, f32) {
    (x + w / 2.0, y + hh / 2.0)
}

fn key(h: &Harness<RemoteFolderPreview>, k: Key) {
    let c: char = k.into();
    h.key_text(&c.to_string());
    h.advance(0);
}

/// Rule 1282: a click on a folder picks it, a click on a file does nothing, a double-click goes in; a crumb goes
/// there; Back (only when there is somewhere to go) and Home answer, each with its tip; Escape and a press outside
/// cancel, Enter is Open here, the arrows step; nothing behind the dialog is ever reached.
#[test]
fn the_pointer_and_the_keys_reach_the_dialog_and_never_what_is_behind() {
    for s in [1.0, 1.5, 2.25] {
        let h = harness(s);
        open(&h);
        assert!(h.comp.get_keys_held(), "{s}: the dialog has the keys");
        let (lx, ly, lw) = (
            h.comp.get_list_x(),
            h.comp.get_list_y(),
            h.comp.get_list_w(),
        );
        let row = |i: f32| (lx + lw / 2.0, ly + 1.0 + i * 28.0 + 14.0);
        let (x, y) = row(1.0);
        h.click(x, y);
        h.advance(500);
        assert_eq!(h.comp.get_picked(), 1, "{s}: a click picks the folder");
        let (x, y) = row(6.0);
        h.click(x, y);
        h.advance(500);
        assert_eq!(h.comp.get_picked(), 1, "{s}: a file is never picked");
        let (x, y) = row(3.0);
        h.click(x, y);
        h.advance(30);
        h.click(x, y);
        h.advance(500);
        assert_eq!(h.comp.get_entered(), 3, "{s}: a double-click goes in");
        // Back waits for somewhere to go; Home always answers. Each says what it is on hover.
        let b = h.comp.get_button_size() / 2.0;
        let (bx, by) = (h.comp.get_back_x() + b, h.comp.get_back_y() + b);
        h.click(bx, by);
        assert_eq!(h.comp.get_backs(), 0, "{s}: no Back yet");
        h.comp.set_can_back(true);
        h.advance(0);
        h.move_to(bx, by);
        h.advance(16);
        assert_eq!(h.comp.get_tip_text().as_str(), "Back");
        h.click(bx, by);
        assert_eq!(h.comp.get_backs(), 1, "{s}: Back");
        let (hx, hy) = (h.comp.get_home_x() + b, h.comp.get_home_y() + b);
        h.move_to(hx, hy);
        h.advance(16);
        assert_eq!(h.comp.get_tip_text().as_str(), "Home folder");
        h.click(hx, hy);
        assert_eq!(h.comp.get_homes(), 1, "{s}: Home");
        // A crumb: the one after Home's machine name sits right of the buttons.
        let (ox, oy, ow, oh) = (
            h.comp.get_open_x(),
            h.comp.get_open_y(),
            h.comp.get_open_w(),
            h.comp.get_open_h(),
        );
        let (x, y) = centre(ox, oy, ow, oh);
        h.click(x, y);
        assert_eq!(h.comp.get_opens(), 1, "{s}: Open here");
        key(&h, Key::Return);
        assert_eq!(h.comp.get_opens(), 2, "{s}: Enter is Open here");
        key(&h, Key::DownArrow);
        key(&h, Key::DownArrow);
        key(&h, Key::UpArrow);
        assert_eq!(h.comp.get_steps(), 1, "{s}: the arrows step");
        key(&h, Key::Backspace);
        assert_eq!(h.comp.get_backs(), 2, "{s}: Backspace is Back");
        key(&h, Key::Escape);
        assert_eq!(h.comp.get_cancels(), 1, "{s}: Escape cancels");
        let (cx, cy) = centre(
            h.comp.get_cancel_x(),
            h.comp.get_cancel_y(),
            h.comp.get_cancel_w(),
            h.comp.get_cancel_h(),
        );
        h.click(cx, cy);
        assert_eq!(h.comp.get_cancels(), 2, "{s}: Cancel");
        h.click(20.0, 20.0);
        assert_eq!(h.comp.get_cancels(), 3, "{s}: a press outside cancels");
        assert_eq!(
            h.comp.get_behind_clicks(),
            0,
            "{s}: nothing behind is reached"
        );
    }
}

/// The crumbs: a click on a live one goes there; the last (the folder shown) and a `…` go nowhere.
#[test]
fn a_crumb_goes_to_its_folder_and_the_last_goes_nowhere() {
    let h = harness(1.0);
    open(&h);
    let b = h.comp.get_button_size();
    // The crumbs start 6 px after Home; walk right along the bar and click each label once.
    let y = h.comp.get_home_y() + b / 2.0;
    let start = h.comp.get_home_x() + b + 8.0;
    let mut seen = Vec::new();
    let mut x = start;
    while x < h.comp.get_box_x() + h.comp.get_box_w() - 20.0 {
        h.comp.set_crumbed(-1);
        h.click(x, y);
        let c = h.comp.get_crumbed();
        if c >= 0 && seen.last() != Some(&c) {
            seen.push(c);
        }
        x += 3.0;
    }
    assert_eq!(
        seen,
        [0, 1, 2],
        "every crumb but the last answers, in order"
    );
}

/// Rule 1348: crumbs that fit start at the left as before; a path too long for the row keeps its end (the current
/// folder) in view and cuts the start instead, at every scale.
#[test]
fn a_long_path_keeps_the_current_folder_in_view() {
    for s in SCALES {
        let h = harness(s);
        open(&h);
        let (bx, bw) = (h.comp.get_crumbs_x(), h.comp.get_crumbs_w());
        assert!(
            (h.comp.get_crumb_row_x() - bx).abs() < 0.5,
            "scale {s}: short crumbs moved"
        );
        assert!(h.comp.get_crumb_row_w() < bw, "scale {s}");
        h.comp.set_crumbs(crumbs(&[
            "home-pc",
            "\u{2026}",
            "a-rather-long-project-folder",
            "another-long-folder-name",
            "the-current-folder-with-a-long-name",
        ]));
        h.advance(0);
        h.render();
        let (x, w) = (h.comp.get_crumb_row_x(), h.comp.get_crumb_row_w());
        assert!(
            w > bw,
            "scale {s}: the path should be too long ({w} in {bw})"
        );
        assert!(
            (x + w - (bx + bw)).abs() < 0.5,
            "scale {s}: the row ends at {} but the box at {}",
            x + w,
            bx + bw
        );
    }
}
