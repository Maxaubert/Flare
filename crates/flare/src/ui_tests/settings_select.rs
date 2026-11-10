//! UI tests of the select and its menu (M7 Task 33); headless only. The preview is a 400 by 420
//! window: the trigger at (20, 20), its menu right-aligned 4 px below it (place_menu's rule).

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{SelectPreview, SetOption};

/// `Theme.ground`: the preview's window colour.
const GROUND: [u8; 4] = [23, 23, 23, 255];
/// `Theme.side-flat`: the menu's opaque fill.
const MENU: [u8; 4] = [31, 31, 30, 255];

fn opt(id: &str, name: &str, face: &str) -> SetOption {
    SetOption {
        id: id.into(),
        name: name.into(),
        face: face.into(),
    }
}

fn make(scale: f32, options: Vec<SetOption>, value: &str) -> Harness<SelectPreview> {
    let h = Harness::new(scale, 400, 420, || {
        SelectPreview::new().expect("the preview builds")
    });
    h.comp.set_options(ModelRc::new(VecModel::from(options)));
    h.comp.set_value(value.into());
    h.advance(0);
    h.render();
    h
}

fn three(scale: f32, value: &str) -> Harness<SelectPreview> {
    make(
        scale,
        vec![
            opt("a", "Alpha", ""),
            opt("b", "Bravo", ""),
            opt("c", "Charlie", ""),
        ],
        value,
    )
}

/// Opens the menu from the model side (Rust does this on `open-request`) and lets it settle.
fn open(h: &Harness<SelectPreview>) -> Frame {
    h.comp.set_open(true);
    h.advance(0);
    h.render();
    h.advance(0);
    h.render()
}

fn key(h: &Harness<SelectPreview>, k: Key) {
    h.key_text(&String::from(char::from(k)));
    h.advance(0);
}

fn whole(v: f32, s: f32) -> bool {
    let d = v * s;
    (d - d.round()).abs() < 1e-2
}

fn px(v: f32, s: f32) -> u32 {
    (v * s).round() as u32
}

/// The accent-hi ink (#f6a25b) as a pixel test: a warm, saturated pixel (the soft text is grey).
fn warm(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] > 120 && p[1] < 210 && p[2] < 150 && p[0] as i32 - p[2] as i32 > 80
}

#[test]
fn trigger_is_32_high_and_at_least_168_wide() {
    for s in SCALES {
        let h = three(s, "a");
        let c = &h.comp;
        assert!(
            (c.get_trig_h() * s - 32.0 * s).abs() <= 1.0,
            "scale {s}: height {}",
            c.get_trig_h()
        );
        assert!(c.get_trig_w() * s >= 168.0 * s - 1.0, "scale {s}: width");
        assert!(whole(c.get_trig_x(), s) && whole(c.get_trig_w(), s));
        // a wider ask is honoured, a narrower one is not
        c.set_want_w(240.0);
        assert!((c.get_trig_w() * s - 240.0 * s).abs() <= 1.0, "scale {s}");
        c.set_want_w(100.0);
        assert!((c.get_trig_w() * s - 168.0 * s).abs() <= 1.0, "scale {s}");
    }
}

#[test]
fn the_chosen_option_wears_the_accent_hi_ink_and_a_check() {
    for s in [1.0f32, 2.0] {
        let h = three(s, "b");
        let f = open(&h);
        let c = &h.comp;
        let (mx, my, mw) = (c.get_menu_x(), c.get_menu_y(), c.get_menu_w());
        let row = |i: usize| {
            let top = my + 2.0 + 28.0 * i as f32;
            (px(mx, s), px(top, s), px(mx + mw, s), px(top + 28.0, s))
        };
        let count = |i: usize, x0: u32, x1: u32| {
            let r = row(i);
            f.count_where(x0, r.1, x1, r.3, warm)
        };
        let chosen = row(1);
        let mid = px(mx + mw - 30.0, s);
        assert!(count(1, chosen.0, mid) > 0, "scale {s}: the text is warm");
        assert!(count(1, mid, chosen.2) > 0, "scale {s}: the check is warm");
        assert_eq!(count(0, row(0).0, row(0).2), 0, "scale {s}: row 0 is plain");
        assert_eq!(count(2, row(2).0, row(2).2), 0, "scale {s}: row 2 is plain");
    }
}

#[cfg_attr(
    not(windows),
    ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
)]
#[test]
fn the_font_options_are_drawn_in_their_own_face() {
    for s in SCALES {
        let h = make(
            s,
            vec![
                opt("mono", "WWWWiiiilll", "Consolas"),
                opt("ui", "WWWWiiiilll", "Segoe UI"),
            ],
            "",
        );
        let f = open(&h);
        let c = &h.comp;
        let (mx, my, mw) = (c.get_menu_x(), c.get_menu_y(), c.get_menu_w());
        let ink = |i: usize| {
            let top = my + 2.0 + 28.0 * i as f32;
            f.bbox_where(
                px(mx + 4.0, s),
                px(top, s),
                px(mx + mw - 30.0, s),
                px(top + 28.0, s),
                |p| !Frame::near(p, MENU, 12),
            )
        };
        let a = ink(0).expect("row 0 has text");
        let b = ink(1).expect("row 1 has text");
        assert_ne!(
            a.2 - a.0,
            b.2 - b.0,
            "scale {s}: the two faces draw different widths"
        );
    }
}

#[test]
fn escape_closes_and_is_consumed() {
    let h = three(1.0, "a");
    let closed = Rc::new(RefCell::new(0));
    let keys = Rc::new(RefCell::new(Vec::<String>::new()));
    let (c2, k2) = (closed.clone(), keys.clone());
    h.comp.on_closed(move || *c2.borrow_mut() += 1);
    h.comp
        .on_menu_key(move |k| k2.borrow_mut().push(k.to_string()));
    open(&h);
    key(&h, Key::Escape);
    assert_eq!(*closed.borrow(), 1);
    assert_eq!(*keys.borrow(), vec!["Escape".to_string()]);
    assert!(!h.comp.get_open());
    assert_eq!(h.comp.get_leaked(), 0, "Escape did not reach the window");
}

#[test]
fn a_click_outside_dismisses() {
    let h = three(1.0, "a");
    let closed = Rc::new(RefCell::new(0));
    let c2 = closed.clone();
    h.comp.on_closed(move || *c2.borrow_mut() += 1);
    open(&h);
    // inside the menu (its padding and an option) keeps it open until an option is chosen
    let c = &h.comp;
    let (mx, my) = (c.get_menu_x(), c.get_menu_y());
    h.click(mx + 40.0, my + 0.5);
    assert_eq!(*closed.borrow(), 0, "a press on the menu edge is absorbed");
    assert!(h.comp.get_open());
    h.click(350.0, 380.0);
    assert_eq!(*closed.borrow(), 1);
    assert!(!h.comp.get_open());
}

#[test]
fn up_down_enter_choose() {
    let h = three(1.0, "a");
    let picked = Rc::new(RefCell::new(Vec::<String>::new()));
    let p2 = picked.clone();
    h.comp
        .on_picked(move |id| p2.borrow_mut().push(id.to_string()));
    open(&h);
    assert_eq!(h.comp.get_highlight(), 0, "opens on the chosen option");
    key(&h, Key::DownArrow);
    assert_eq!(h.comp.get_highlight(), 1);
    key(&h, Key::DownArrow);
    key(&h, Key::DownArrow);
    assert_eq!(h.comp.get_highlight(), 2, "clamped at the end");
    key(&h, Key::UpArrow);
    assert_eq!(h.comp.get_highlight(), 1);
    key(&h, Key::Home);
    assert_eq!(h.comp.get_highlight(), 0);
    key(&h, Key::End);
    assert_eq!(h.comp.get_highlight(), 2);
    key(&h, Key::UpArrow);
    key(&h, Key::Return);
    assert_eq!(*picked.borrow(), vec!["b".to_string()]);
    assert!(!h.comp.get_open(), "choosing closes the menu");
    // Space chooses too
    open(&h);
    assert_eq!(h.comp.get_highlight(), 1);
    key(&h, Key::DownArrow);
    key(&h, Key::Space);
    assert_eq!(*picked.borrow(), vec!["b".to_string(), "c".to_string()]);
    // a click on an option chooses it
    open(&h);
    let c = &h.comp;
    h.click(c.get_menu_x() + 40.0, c.get_menu_y() + 2.0 + 14.0);
    assert_eq!(picked.borrow().last().map(String::as_str), Some("a"));
}

#[test]
fn the_trigger_opens_from_the_keys_and_the_pointer() {
    let h = three(1.0, "a");
    let opened = Rc::new(RefCell::new(0));
    let o2 = opened.clone();
    h.comp.on_opened(move || *o2.borrow_mut() += 1);
    for k in [Key::Return, Key::Space, Key::DownArrow] {
        h.comp.set_open(false);
        h.advance(0);
        h.comp.invoke_focus_trigger();
        key(&h, k);
        assert!(h.comp.get_open(), "{k:?} opens");
    }
    assert_eq!(*opened.borrow(), 3);
    h.comp.set_open(false);
    h.advance(0);
    h.click(100.0, 36.0);
    assert!(h.comp.get_open(), "a click opens");
    // a disabled trigger does nothing
    h.comp.set_open(false);
    h.comp.set_disabled(true);
    h.advance(0);
    h.click(100.0, 36.0);
    assert!(!h.comp.get_open());
}

#[test]
fn a_long_list_scrolls_inside_300() {
    for s in SCALES {
        let opts: Vec<SetOption> = (0..15)
            .map(|i| opt(&format!("o{i}"), &format!("Option {i}"), ""))
            .collect();
        let h = make(s, opts, "o0");
        let c = &h.comp;
        assert!(
            (c.get_menu_h() * s - 300.0 * s).abs() <= 1.0,
            "scale {s}: menu-h {}",
            c.get_menu_h()
        );
        assert!(whole(c.get_menu_y(), s) && whole(c.get_menu_h(), s));
    }
    // a short list is its content: 3 rows of 28 and 2 px above and below
    let h = three(1.0, "a");
    assert!((h.comp.get_menu_h() - 88.0).abs() < 0.5);
}

#[test]
fn the_menu_has_no_shadow_and_the_row_radius() {
    for s in [1.0f32, 2.0] {
        let h = three(s, "a");
        let f = open(&h);
        let c = &h.comp;
        let (mx, my, mw, mh) = (
            c.get_menu_x(),
            c.get_menu_y(),
            c.get_menu_w(),
            c.get_menu_h(),
        );
        // the corner pixel lies outside the 7 px arc: still the window colour
        let corner = f.px(px(mx, s), px(my, s));
        assert!(
            Frame::near(corner, GROUND, 4),
            "scale {s}: corner {corner:?}"
        );
        // a cast shadow would darken the ground just below and beside the menu
        let below = f.px(px(mx + mw / 2.0, s), px(my + mh, s) + 2);
        assert!(Frame::near(below, GROUND, 2), "scale {s}: below {below:?}");
        let beside = f.px(px(mx, s).saturating_sub(2), px(my + mh / 2.0, s));
        assert!(
            Frame::near(beside, GROUND, 2),
            "scale {s}: beside {beside:?}"
        );
        // inside, the fill is opaque side-flat
        let inside = f.px(px(mx + mw / 2.0, s), px(my + mh - 3.0, s));
        assert!(
            Frame::near(inside, MENU, 10),
            "scale {s}: inside {inside:?}"
        );
    }
}

/// The mean row (device px, from the box's top) of the chevron's ink in its middle columns.
fn chevron_apex_row(h: &Harness<SelectPreview>, f: &Frame) -> f32 {
    let s = h.scale();
    let c = &h.comp;
    // The chevron: 12 px, 10 px in from the trigger's right edge, centred on its height.
    let x0 = c.get_trig_x() + c.get_trig_w() - 22.0;
    let y0 = c.get_trig_y() + (c.get_trig_h() - 12.0) / 2.0;
    let (cx0, cx1) = (px(x0 + 5.0, s), px(x0 + 7.0, s));
    let (ry0, ry1) = (px(y0, s), px(y0 + 12.0, s));
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in ry0..ry1 {
        for x in cx0..cx1 {
            let p = f.px(x, y);
            if p[0] > 70 {
                sum += (y - ry0) as f32;
                n += 1.0;
            }
        }
    }
    assert!(n > 0.0, "the chevron is drawn");
    sum / n / s
}

#[test]
fn the_chevron_turns_over_while_the_menu_is_open() {
    // PT: `rotate-180` on the chevron while open (Task 53: Flare's stayed pointing down).
    for s in SCALES {
        let h = three(s, "a");
        let shut = chevron_apex_row(&h, &h.render());
        h.comp.set_open(true);
        h.advance(400);
        let open = chevron_apex_row(&h, &h.render());
        // The point sits 6 of 12 px from one edge of the box or the other.
        assert!(
            shut - open > 0.8,
            "scale {s}: the point's row closed {shut:.1}, open {open:.1}"
        );
    }
}

#[test]
fn the_chevron_is_drawn_at_its_full_12_px() {
    // "M6 9l6 6 6-6" in a 12 px box spans 6 css px, plus a round cap of half the 1.1 px stroke at
    // each end: 7.1 wide, as Chromium draws it. Slint insets a stroked path by its stroke, which
    // drew it at 6 (Task 53).
    for s in [2.0f32, 3.0] {
        let h = three(s, "a");
        let f = h.render();
        let c = &h.comp;
        let x0 = c.get_trig_x() + c.get_trig_w() - 26.0;
        let y0 = c.get_trig_y() + 4.0;
        let bb = f
            .bbox_where(
                px(x0, s),
                px(y0, s),
                px(x0 + 20.0, s),
                px(y0 + 24.0, s),
                |p| p[0] > 60,
            )
            .expect("the chevron is drawn");
        let w = (bb.2 - bb.0 + 1) as f32 / s;
        assert!(
            (6.6..=7.6).contains(&w),
            "scale {s}: the chevron is {w:.2} css px wide"
        );
    }
}

/// The ink of a word in a box: pixels past half way from the box's left-edge fill to its brightest.
fn word_ink(f: &Frame, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    let bg = i32::from(f.px(x0, (y0 + y1) / 2)[0]);
    let mut peak = bg;
    for y in y0..y1 {
        for x in x0..x1 {
            peak = peak.max(i32::from(f.px(x, y)[0]));
        }
    }
    let cut = bg + (peak - bg) / 2;
    f.count_where(x0, y0, x1, y1, |p| i32::from(p[0]) > cut)
}

#[test]
fn the_trigger_word_is_semibold_and_the_menu_rows_regular() {
    // PT: the trigger is font-medium (500), an option row the normal weight. Segoe UI has no Medium
    // face and Chromium draws 500 with the Semibold one (Task 53), so the trigger's word is visibly
    // heavier than the same word in the menu.
    for s in SCALES {
        let h = make(s, vec![opt("a", "Bravo", ""), opt("b", "Bravo", "")], "a");
        let f = open(&h);
        let c = &h.comp;
        let (tx, ty) = (c.get_trig_x(), c.get_trig_y());
        // The word sits from 10 px in; 60 px holds "Bravo" at 12 px.
        let trig = word_ink(
            &f,
            px(tx + 4.0, s),
            px(ty + 4.0, s),
            px(tx + 70.0, s),
            px(ty + 28.0, s),
        );
        let (mx, my) = (c.get_menu_x(), c.get_menu_y());
        let top = my + 2.0 + 28.0;
        let row = word_ink(
            &f,
            px(mx + 4.0, s),
            px(top + 2.0, s),
            px(mx + 72.0, s),
            px(top + 26.0, s),
        );
        assert!(
            trig > 0 && row > 0,
            "scale {s}: both words drawn ({trig}, {row})"
        );
        let ratio = trig as f32 / row as f32;
        assert!(
            ratio > 1.12,
            "scale {s}: the trigger word has {trig} ink pixels, the menu row {row} (ratio {ratio:.2})"
        );
    }
}
