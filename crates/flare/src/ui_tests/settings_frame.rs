//! UI tests of the rail, the header and the pane frame (M7 Task 35); headless only.
//!
//! `FramePreview` is the whole window: the frame fills it, six rail items (About last), a tall page
//! content. Its out properties are the frame's own geometry in logical px.

use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, SharedString, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{FramePreview, SetHit};

/// `Theme.side` and `Theme.ground` (pt-default, the same grey).
const GROUND: [u8; 4] = [23, 23, 23, 255];
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];

fn make(scale: f32, w: u32, h: u32) -> Harness<FramePreview> {
    let hn = Harness::new(scale, w, h, || {
        FramePreview::new().expect("the preview builds")
    });
    hn.advance(0);
    hn
}

fn wide(scale: f32) -> Harness<FramePreview> {
    make(scale, 1000, 604)
}

fn dev(scale: f32, css: f32) -> u32 {
    (css * scale).round() as u32
}

fn whole(v: f32, s: f32) -> bool {
    let d = v * s;
    (d - d.round()).abs() < 1e-2
}

fn key(h: &Harness<FramePreview>, k: Key) {
    h.key_text(&String::from(char::from(k)));
}

fn hits(n: usize) -> ModelRc<SetHit> {
    let rows: Vec<SetHit> = (0..n)
        .map(|i| SetHit {
            id: SharedString::from(format!("h{i}")),
            icon: SharedString::from("M4 6h16v12H4z"),
            label: SharedString::from(format!("Setting {i}")),
            sub: SharedString::from("A line."),
            r#where: SharedString::from("Terminal, Shell"),
            selected: false,
            y: 58.0 * i as f32,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

#[test]
fn rail_244_and_find_32_and_items_36_at_every_scale() {
    for s in SCALES {
        let h = wide(s);
        let c = &h.comp;
        assert!((c.get_rail_w() - 244.0).abs() < 0.01, "scale {s}");
        assert!((c.get_pane_x() - 244.0).abs() < 0.01, "scale {s}");
        assert!((c.get_pane_w() - 756.0).abs() < 0.01, "scale {s}");
        for e in [
            c.get_find_y(),
            c.get_find_y() + c.get_find_h(),
            c.get_header_y(),
            c.get_content_top(),
        ] {
            assert!(whole(e, s), "scale {s}: edge {e} off a device pixel");
        }
        assert!(
            (c.get_find_h() * s - 32.0 * s).abs() < 0.01,
            "scale {s}: find height {}",
            c.get_find_h()
        );
        assert!((c.get_item_w() - 219.0).abs() <= 1.0 / s, "scale {s}");
        // Pixels: the find box (border + fill) differs from the rail fill; it is 219 by 32.
        let f = h.render();
        let bb = f
            .bbox_where(0, dev(s, 44.0), dev(s, 243.0), dev(s, 92.0), |p| {
                p != GROUND
            })
            .expect("the find field is drawn");
        assert_eq!(bb.2 - bb.0 + 1, dev(s, 219.0), "scale {s}: find width");
        assert_eq!(bb.3 - bb.1 + 1, dev(s, 32.0), "scale {s}: find height");
        // The chosen item (Appearance) is a 219 by 36 grey fill.
        let fill = f.px(dev(s, 200.0), dev(s, 117.0));
        assert_ne!(fill, GROUND, "scale {s}: the chosen item has a fill");
        let ib = f
            .bbox_where(0, dev(s, 94.0), dev(s, 243.0), dev(s, 136.0), |p| {
                Frame::near(p, fill, 1)
            })
            .expect("the chosen item is drawn");
        assert_eq!(ib.2 - ib.0 + 1, dev(s, 219.0), "scale {s}: item width");
        assert_eq!(ib.3 - ib.1 + 1, dev(s, 36.0), "scale {s}: item height");
    }
}

#[test]
fn the_chosen_page_is_grey_never_the_accent() {
    for s in [1.0, 1.5, 2.25] {
        let h = wide(s);
        let f = h.render();
        let fill = f.px(dev(s, 200.0), dev(s, 117.0));
        // The hover-hi white at 11 percent over the rail grey is about 48.
        for ch in fill.iter().take(3) {
            assert!(
                (i32::from(*ch) - 48).abs() <= 2,
                "scale {s}: chosen fill {fill:?}"
            );
        }
        assert_eq!(
            f.count_where(0, 0, f.w, f.h, |p| Frame::near(p, ACCENT, 24)),
            0,
            "scale {s}: no accent anywhere in the frame"
        );
        // Searching: no item is chosen, so that spot is the rail's own fill again.
        h.comp.set_searching(true);
        h.advance(0);
        assert_eq!(
            h.render().px(dev(s, 200.0), dev(s, 117.0)),
            GROUND,
            "scale {s}: none chosen while searching"
        );
    }
}

#[test]
fn below_760_the_rail_folds_to_56_and_find_to_a_magnifier() {
    for s in [1.0, 1.25, 2.0] {
        let w = make(s, 759, 604);
        assert!((w.comp.get_rail_w() - 56.0).abs() < 0.01, "scale {s}: 759");
        assert!((w.comp.get_find_w() - 40.0).abs() < 0.01, "scale {s}");
        assert!((w.comp.get_item_w() - 40.0).abs() < 0.01, "scale {s}");
        assert!((w.comp.get_content_x() - 20.0).abs() < 0.01, "scale {s}");
        let n = make(s, 760, 604);
        assert!((n.comp.get_rail_w() - 244.0).abs() < 0.01, "scale {s}: 760");
        assert!((n.comp.get_find_w() - 219.0).abs() <= 1.0 / s, "scale {s}");
        assert!((n.comp.get_content_x() - 44.0).abs() < 0.01, "scale {s}");
    }
    for width in [900u32, 1600] {
        let h = make(1.0, width, 604);
        let c = &h.comp;
        let w = width as f32;
        assert!((c.get_rail_w() + c.get_pane_w() - w).abs() < 0.01);
        assert!(
            c.get_content_x() + 960.0f32.min(c.get_pane_w()) - 2.0 * 44.0 + 44.0
                <= c.get_pane_w() + 0.01,
            "{width}: no horizontal overflow"
        );
    }
}

#[test]
fn the_narrow_field_opens_300_wide_on_focus_and_with_text() {
    for s in [1.0, 1.5] {
        let h = make(s, 700, 604);
        assert!(
            (h.comp.get_find_w() - 40.0).abs() < 0.01,
            "scale {s}: folded"
        );
        h.comp.invoke_focus_field();
        h.advance(0);
        assert!(
            (h.comp.get_find_w() - 300.0).abs() < 0.01,
            "scale {s}: focused"
        );
        // It lies over the pane: the right part of the box is the side-flat fill, not the page ground.
        let f = h.render();
        let over = f.px(dev(s, 280.0), dev(s, 70.0));
        assert_ne!(
            over, GROUND,
            "scale {s}: the open field is drawn over the pane"
        );
    }
    // Holding text keeps it open without the focus.
    let h = make(1.0, 700, 604);
    h.comp.set_find_text("ab".into());
    h.advance(0);
    assert!((h.comp.get_find_w() - 300.0).abs() < 0.01);
    assert!(h.comp.get_clear_visible());
}

#[test]
fn results_replace_the_page_and_the_status_line_appears() {
    for s in [1.0, 2.0] {
        let h = wide(s);
        let x0 = dev(s, 244.0 + 44.0);
        let band = (dev(s, 26.0), dev(s, 54.0));
        let ink =
            |f: &Frame, x0: u32, x1: u32| f.bbox_where(x0, band.0, x1, band.1, |p| p[0] > 120);
        let page = ink(&h.render(), x0, dev(s, 1000.0)).expect("the title is drawn");
        h.comp.set_searching(true);
        h.advance(0);
        let f = h.render();
        let res = ink(&f, x0, dev(s, 1000.0)).expect("the header is drawn");
        // "Results" is a different word from "Appearance": the ink differs, and the status line
        // sits at the far right of the column (the column ends at css 244 + 44 + 668 = 956).
        assert_ne!(
            (page.0, page.2),
            (res.0, res.2),
            "scale {s}: the header changed"
        );
        assert!(
            res.2 > dev(s, 900.0),
            "scale {s}: status line at the right, ink ends {}",
            res.2
        );
        h.comp.set_searching(false);
        h.advance(0);
        let back = ink(&h.render(), x0, dev(s, 1000.0)).expect("title");
        assert!(
            back.2 < dev(s, 700.0),
            "scale {s}: no status line off search"
        );
    }
}

#[test]
fn no_hits_shows_the_nothing_text() {
    for s in [1.0, 2.0] {
        let h = wide(s);
        h.comp.set_searching(true);
        h.advance(0);
        let f = h.render();
        let top = dev(s, 72.0);
        // The panel is drawn: its fill differs from the ground below the header.
        let panel = f
            .bbox_where(dev(s, 250.0), top, dev(s, 990.0), dev(s, 200.0), |p| {
                p != GROUND
            })
            .expect("the nothing panel is drawn");
        assert!(
            panel.3 - panel.1 + 1 >= dev(s, 60.0),
            "scale {s}: panel height"
        );
        // The query word is in the full text ink (semibold), the rest is dim.
        let ink = f.count_where(dev(s, 250.0), top, dev(s, 990.0), dev(s, 200.0), |p| {
            Frame::near(p, [0xde, 0xda, 0xcf, 255], 6)
        });
        assert!(ink > 0, "scale {s}: the query is in the text ink");
    }
}

#[test]
fn the_header_is_24_px_bold_with_the_title_or_results() {
    for s in [1.0, 1.5, 2.0] {
        let h = wide(s);
        let f = h.render();
        let bb = f
            .bbox_where(
                dev(s, 288.0),
                dev(s, 20.0),
                dev(s, 700.0),
                dev(s, 60.0),
                |p| p[0] > 120,
            )
            .expect("the title is drawn");
        let ink_h = (bb.3 - bb.1 + 1) as f32 / s;
        assert!(
            (15.0..=27.0).contains(&ink_h),
            "scale {s}: header ink {ink_h} css px high"
        );
        h.comp.set_searching(true);
        h.advance(0);
        let r = h
            .render()
            .bbox_where(
                dev(s, 288.0),
                dev(s, 20.0),
                dev(s, 600.0),
                dev(s, 60.0),
                |p| p[0] > 120,
            )
            .expect("Results is drawn");
        let cap = (r.3 - r.1 + 1) as f32 / s;
        assert!(
            (14.0..=22.0).contains(&cap),
            "scale {s}: Results cap height {cap}"
        );
    }
}

#[test]
fn clear_button_only_with_text() {
    let h = wide(1.0);
    assert!(!h.comp.get_clear_visible());
    h.comp.set_find_text("a".into());
    h.advance(0);
    assert!(h.comp.get_clear_visible());
    // A press on the clear button (right 6, 20 wide: its centre is 16 in from the field's right
    // edge, at css 12 + 219 - 16 = 215, y 52.5 + 16) empties the field and reports it.
    h.click(215.0, 69.0);
    assert_eq!(h.comp.get_find_text(), "");
    assert_eq!(h.comp.get_edited_text(), "");
    assert!(!h.comp.get_clear_visible());
}

#[test]
fn keys_reach_their_handlers() {
    let h = wide(1.0);
    // Escape on an empty field is the host's.
    h.comp.invoke_focus_field();
    h.advance(0);
    key(&h, Key::Escape);
    assert_eq!(h.comp.get_escape_unhandled_count(), 1);
    assert_eq!(h.comp.get_key_count(), 0);
    // Typing fills the field; Escape with text is consumed and reported.
    h.key_text("ab");
    assert_eq!(h.comp.get_find_text(), "ab");
    key(&h, Key::Escape);
    assert_eq!(h.comp.get_last_key(), "find:Escape");
    assert_eq!(
        h.comp.get_escape_unhandled_count(),
        1,
        "consumed, not passed on"
    );
    // Down and Enter reach Rust only when there are hits.
    let n = h.comp.get_key_count();
    key(&h, Key::DownArrow);
    key(&h, Key::Return);
    assert_eq!(h.comp.get_key_count(), n, "no hits, no forwarding");
    h.comp.set_searching(true);
    h.comp.set_hits(hits(3));
    h.advance(0);
    key(&h, Key::DownArrow);
    assert_eq!(h.comp.get_last_key(), "find:Down");
    key(&h, Key::Return);
    assert_eq!(h.comp.get_last_key(), "find:Enter");
    // The list.
    h.comp.invoke_focus_list();
    h.advance(0);
    for (k, want) in [
        (Key::DownArrow, "hit:Down"),
        (Key::UpArrow, "hit:Up"),
        (Key::Return, "hit:Enter"),
        (Key::Escape, "hit:Escape"),
    ] {
        key(&h, k);
        assert_eq!(h.comp.get_last_key(), want);
    }
    h.key_text(" ");
    assert_eq!(h.comp.get_last_key(), "hit:Space");
    // The rail.
    h.comp.set_searching(false);
    h.comp.set_focus_rail_index(2);
    h.advance(0);
    for (k, want) in [
        (Key::DownArrow, "rail:Down"),
        (Key::UpArrow, "rail:Up"),
        (Key::Home, "rail:Home"),
        (Key::End, "rail:End"),
    ] {
        key(&h, k);
        assert_eq!(h.comp.get_last_key(), want);
    }
    key(&h, Key::Return);
    assert_eq!(h.comp.get_page_picked(), "agents");
}

#[test]
fn a_press_on_a_rail_item_and_on_a_hit_is_reported() {
    let h = wide(1.0);
    h.click(100.0, 155.0); // Terminal: y 136.5..172.5
    assert_eq!(h.comp.get_page_picked(), "terminal");
    h.comp.set_searching(true);
    h.comp.set_hits(hits(3));
    h.advance(0);
    // The panel starts at content-top (css 72); row 2 is 58 further down.
    h.click(500.0, h.comp.get_content_top() + 1.0 + 58.0 + 20.0);
    assert_eq!(h.comp.get_hit_picked(), 1);
}

#[test]
fn tab_walks_the_field_then_the_rail_then_the_page() {
    let h = wide(1.0);
    h.comp.invoke_focus_field();
    h.advance(0);
    assert!(h.comp.get_field_focused());
    h.key_with(false, false, &String::from(char::from(Key::Tab)));
    key(&h, Key::DownArrow);
    assert_eq!(
        h.comp.get_last_key(),
        "rail:Down",
        "the first rail item follows the field"
    );
    for _ in 0..5 {
        h.key_with(false, false, &String::from(char::from(Key::Tab)));
    }
    assert!(
        !h.comp.get_content_focused(),
        "six items are not yet the page"
    );
    h.key_with(false, false, &String::from(char::from(Key::Tab)));
    assert!(
        h.comp.get_content_focused(),
        "the page follows the last rail item"
    );
    h.key_with(true, false, &String::from(char::from(Key::Tab)));
    assert!(!h.comp.get_content_focused(), "Shift+Tab walks back");
}

#[test]
fn the_pane_scrolls_and_shows_no_bar() {
    for s in [1.0, 2.0] {
        let h = wide(s);
        assert!(h.comp.get_scroll_y().abs() < 0.01);
        h.comp
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position: LogicalPosition::new(600.0, 300.0),
                delta_x: 0.0,
                delta_y: -120.0,
            });
        h.advance(0);
        let y = h.comp.get_scroll_y();
        assert!(y > 0.0, "scale {s}: the wheel moved the pane, got {y}");
        // Setting it from Rust moves the content: the tall block's top edge rises.
        h.comp.set_scroll_y(100.0);
        h.advance(0);
        let f = h.render();
        let block = f
            .bbox_where(dev(s, 300.0), 0, dev(s, 900.0), f.h, |p| {
                Frame::near(p, [0x31, 0x30, 0x2f, 255], 1)
            })
            .expect("the page content is drawn");
        assert_eq!(
            block.1, 0,
            "scale {s}: scrolled 100, the block (top 72) is above the pane's top"
        );
        // No bar: the strip right of the column holds only the ground.
        let strip = f.count_where(dev(s, 970.0), 0, f.w, f.h, |p| p != GROUND);
        assert_eq!(strip, 0, "scale {s}: no scrollbar pixels");
    }
}
