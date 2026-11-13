//! Component and flow tests of the Milestone 5 terminal (Tasks 17 to 25); headless only.

// badge
// The Copied badge (Task 18): drawn at the bottom centre, 24 px up, at every display scaling.

use crate::BadgePreview;
use crate::ui_harness::{Frame, Harness, SCALES};

/// `Theme.ground`, `Theme.control` and `Theme.divider` over the ground (the hairline blends).
const GROUND: [u8; 4] = [23, 23, 23, 255];
const CONTROL: [u8; 4] = [30, 30, 29, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];

fn badge(scale: f32) -> Harness<BadgePreview> {
    Harness::new(scale, 400, 300, || BadgePreview::new().unwrap())
}

fn is_ink(p: [u8; 4]) -> bool {
    !Frame::near(p, GROUND, 2)
}

#[test]
fn the_badge_is_not_drawn_while_gone() {
    let h = badge(1.0);
    h.comp.set_phase(0);
    h.advance(0);
    let f = h.render();
    assert_eq!(f.count_where(0, 0, f.w, f.h, is_ink), 0);
}

#[test]
fn the_pill_is_centred_and_24_px_from_the_bottom_at_every_scale() {
    for s in SCALES {
        let h = badge(s);
        h.comp.set_phase(1);
        h.advance(500);
        let f = h.render();
        let (x0, _, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_ink)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let centre = (x0 + x1 + 1) as f32 / 2.0;
        assert!(
            (centre - f.w as f32 / 2.0).abs() <= 1.0,
            "scale {s}: centre {centre} of width {}",
            f.w
        );
        let bottom = y1 + 1;
        let want = f.h - (24.0 * s).round() as u32;
        assert!(
            bottom.abs_diff(want) <= 1,
            "scale {s}: bottom {bottom}, want {want}"
        );
    }
}

#[test]
fn the_pill_has_a_hairline_border_and_a_fill() {
    for s in [1.0_f32, 2.25] {
        let h = badge(s);
        h.comp.set_phase(1);
        h.advance(500);
        let f = h.render();
        let (x0, y0, x1, y1) = f
            .bbox_where(0, 0, f.w, f.h, is_ink)
            .unwrap_or_else(|| panic!("scale {s}: no pill drawn"));
        let mid = (y0 + y1) / 2;
        let hair = (s.floor().max(1.0)) as u32;
        let border = f.run_len_h(mid, x0, |p| Frame::near(p, DIVIDER, 4));
        assert!(
            border.abs_diff(hair) <= 1,
            "scale {s}: border run {border}, want {hair}"
        );
        let interior = f.px(x0 + hair + 3, mid);
        assert!(
            Frame::near(interior, CONTROL, 2),
            "scale {s}: interior {interior:?}"
        );
        assert!(x1 > x0 && y1 > y0);
    }
}

/// Parity line 48: the badge is neutral and flat, with no shadow (PT CopiedBadge.tsx).
#[test]
fn the_pill_casts_no_shadow() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/badge.slint");
    let src = std::fs::read_to_string(path).expect("badge.slint"); // sync-ok: test code only
    assert!(!src.contains("drop-shadow"), "the Copied badge is flat");
}

#[test]
fn leaving_fades_out() {
    let h = badge(1.0);
    h.comp.set_phase(1);
    h.advance(500);
    h.comp.set_phase(2);
    h.advance(200);
    let f = h.render();
    assert_eq!(f.count_where(0, 0, f.w, f.h, is_ink), 0);
}

// find bar
// The find bar (Task 17): 332 x 28 plus two hairlines, 12 px from the top right, no shadow.

use crate::FindBarPreview;

/// `Theme.side-flat`, and `Theme.hover` blended over it.
const SIDE_FLAT: [u8; 4] = [31, 31, 30, 255];
const HOVER_OVER_SIDE: [u8; 4] = [44, 44, 43, 255];
const BORDER_OVER_SIDE: [u8; 4] = [46, 46, 45, 255];

fn find(scale: f32) -> Harness<FindBarPreview> {
    let h = Harness::new(scale, 800, 400, || FindBarPreview::new().unwrap());
    h.comp.set_open(true);
    h.advance(0);
    h
}

fn not_ground(p: [u8; 4]) -> bool {
    !Frame::near(p, GROUND, 2)
}

fn bar_box(f: &Frame, s: f32) -> (u32, u32, u32, u32) {
    f.bbox_where(0, 0, f.w, f.h, not_ground)
        .unwrap_or_else(|| panic!("scale {s}: no bar drawn"))
}

fn hair_of(s: f32) -> u32 {
    s.floor().max(1.0) as u32
}

#[test]
fn the_bar_sits_12_px_from_the_top_right_at_every_scale() {
    for s in SCALES {
        let h = find(s);
        let f = h.render();
        let (_, y0, x1, _) = bar_box(&f, s);
        let right_gap = f.w - (x1 + 1);
        let want = (12.0 * s).round() as u32;
        assert!(
            right_gap.abs_diff(want) <= 1,
            "scale {s}: right gap {right_gap}, want {want}"
        );
        assert!(
            y0.abs_diff(want) <= 1,
            "scale {s}: top gap {y0}, want {want}"
        );
    }
}

#[test]
fn the_bar_is_332_by_28_plus_two_hairlines() {
    for s in SCALES {
        let h = find(s);
        let f = h.render();
        let (x0, y0, x1, y1) = bar_box(&f, s);
        let hair = hair_of(s);
        let w = x1 + 1 - x0;
        let ht = y1 + 1 - y0;
        let want_w = (332.0 * s).round() as u32 + 2 * hair;
        let want_h = (28.0 * s).round() as u32 + 2 * hair;
        assert!(
            w.abs_diff(want_w) <= 1,
            "scale {s}: width {w}, want {want_w}"
        );
        assert!(
            ht.abs_diff(want_h) <= 1,
            "scale {s}: height {ht}, want {want_h}"
        );
    }
}

#[test]
fn the_border_is_a_hairline_and_the_fill_is_side_flat() {
    for s in [1.0_f32, 1.5, 2.25] {
        let h = find(s);
        let f = h.render();
        let (x0, y0, _, y1) = bar_box(&f, s);
        let mid = (y0 + y1) / 2;
        let hair = hair_of(s);
        let border = f.run_len_h(mid, x0, |p| Frame::near(p, BORDER_OVER_SIDE, 4));
        assert!(
            border.abs_diff(hair) <= 1,
            "scale {s}: border run {border}, want {hair}"
        );
        let interior = f.px(x0 + hair + 2, mid);
        assert!(
            Frame::near(interior, SIDE_FLAT, 2),
            "scale {s}: interior {interior:?}"
        );
    }
}

#[test]
fn there_is_no_shadow() {
    for s in [1.0_f32, 2.0] {
        let h = find(s);
        let f = h.render();
        let (x0, y0, x1, y1) = bar_box(&f, s);
        let mid_y = (y0 + y1) / 2;
        let mid_x = (x0 + x1) / 2;
        for d in [1u32, 3, 6] {
            let samples = [
                f.px(x0 - d, mid_y),
                f.px(mid_x, y1 + d),
                f.px(mid_x, y0 - d),
            ];
            for p in samples {
                assert!(Frame::near(p, GROUND, 1), "scale {s}, {d} px out: {p:?}");
            }
        }
    }
}

#[test]
fn typing_reports_the_query_and_enter_steps() {
    let h = find(1.0);
    h.comp.invoke_focus_input();
    h.key_text("abc");
    assert_eq!(h.comp.get_last_query().as_str(), "abc");
    h.key_text("\n");
    assert_eq!(h.comp.get_next_count(), 1);
    h.key_with(true, false, "\n");
    assert_eq!(h.comp.get_previous_count(), 1);
    assert_eq!(h.comp.get_next_count(), 1);
    h.key_text("\u{1b}");
    assert_eq!(h.comp.get_close_count(), 1);
    assert_eq!(h.comp.get_query().as_str(), "abc");
}

#[test]
fn the_placeholder_is_drawn_only_while_empty() {
    let h = find(1.0);
    let f = h.render();
    let (x0, y0, _, y1) = bar_box(&f, 1.0);
    let in_x0 = x0 + 1 + 8;
    let in_x1 = in_x0 + 180;
    let ink = |p: [u8; 4]| !Frame::near(p, SIDE_FLAT, 8);
    let empty = f
        .bbox_where(in_x0, y0 + 2, in_x1, y1, ink)
        .expect("the placeholder is drawn while empty");
    assert!(empty.2 - empty.0 > 40, "placeholder spans {empty:?}");
    h.comp.set_query("x".into());
    h.advance(0);
    let f = h.render();
    let typed = f.bbox_where(in_x0, y0 + 2, in_x1, y1, ink);
    let width = typed.map_or(0, |b| b.2 - b.0);
    assert!(width < 20, "only the typed letter remains: {typed:?}");
}

#[test]
fn opening_focuses_the_input() {
    let h = find(1.0);
    h.comp.set_open(false);
    h.advance(0);
    h.comp.set_open(true);
    h.advance(0);
    h.key_text("z");
    assert_eq!(h.comp.get_query().as_str(), "z");
}

#[test]
fn the_readout_is_right_aligned_with_a_52_px_minimum() {
    for s in [1.0_f32, 2.0] {
        let h = find(s);
        h.comp.set_readout("3 of 12".into());
        h.advance(0);
        let f = h.render();
        let (x0, y0, _, y1) = bar_box(&f, s);
        let hair = hair_of(s);
        let box_x0 = x0 + hair + ((8.0 + 180.0 + 6.0) * s).round() as u32;
        let box_x1 = box_x0 + (52.0 * s).round() as u32;
        let ink = |p: [u8; 4]| !Frame::near(p, SIDE_FLAT, 8);
        let b = f
            .bbox_where(box_x0, y0 + hair, box_x1, y1 + 1 - hair, ink)
            .unwrap_or_else(|| panic!("scale {s}: no readout ink"));
        let gap = (box_x1 - 1).abs_diff(b.2);
        assert!(
            gap <= s.ceil() as u32 + 1,
            "scale {s}: ink right {} box right {}",
            b.2,
            box_x1 - 1
        );
        h.comp.set_readout("".into());
        h.advance(0);
        let f2 = h.render();
        assert!(
            f2.bbox_where(box_x0, y0 + hair, box_x1, y1 + 1 - hair, ink)
                .is_none(),
            "scale {s}: ink with no readout"
        );
        let (a, _, c, _) = bar_box(&f2, s);
        let want = (332.0 * s).round() as u32 + 2 * hair;
        assert!((c + 1 - a).abs_diff(want) <= 1, "scale {s}: width changed");
    }
}

#[test]
fn previous_and_next_do_nothing_without_hits() {
    let h = find(1.0);
    h.comp.set_can_step(false);
    h.advance(0);
    h.click(718.0, 26.0);
    h.click(744.0, 26.0);
    assert_eq!(
        (h.comp.get_previous_count(), h.comp.get_next_count()),
        (0, 0)
    );
    h.click(770.0, 26.0);
    assert_eq!(h.comp.get_close_count(), 1);
    h.comp.set_can_step(true);
    h.advance(0);
    h.click(718.0, 26.0);
    h.click(744.0, 26.0);
    assert_eq!(
        (h.comp.get_previous_count(), h.comp.get_next_count()),
        (1, 1)
    );
}

#[test]
fn a_hovered_button_lights_a_20_px_square() {
    for s in SCALES {
        let h = find(s);
        h.comp.set_can_step(true);
        h.advance(0);
        h.move_to(744.0, 26.0);
        h.advance(0);
        let f = h.render();
        let (bx0, by0, bx1, by1) = bar_box(&f, s);
        let hair = hair_of(s);
        let b = f
            .bbox_where(
                h.phys(744.0 - 14.0).max(bx0 + hair),
                by0 + hair,
                h.phys(744.0 + 14.0).min(bx1 + 1 - hair),
                by1 + 1 - hair,
                |p| Frame::near(p, HOVER_OVER_SIDE, 2),
            )
            .unwrap_or_else(|| panic!("scale {s}: no hover fill"));
        let want = (20.0 * s).round() as u32;
        assert!(
            (b.2 + 1 - b.0).abs_diff(want) <= 1,
            "scale {s}: width {}",
            b.2 + 1 - b.0
        );
        assert!(
            (b.3 + 1 - b.1).abs_diff(want) <= 1,
            "scale {s}: height {}",
            b.3 + 1 - b.1
        );
    }
}

#[test]
fn no_hex_colour_literal_in_find_slint() {
    let text = include_str!("../../ui/find.slint");
    for (i, _) in text.match_indices('#') {
        let run: String = text[i + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        let lower = run.to_ascii_lowercase();
        assert!(
            run.is_empty() || lower.starts_with("ffffff") || lower.starts_with("000000"),
            "find.slint holds the colour #{run}"
        );
    }
}

// grid area
// The grid pointer area, the find bar and the badge in the assembled window (Task 19).

mod grid_area {
    use std::cell::RefCell;
    use std::rc::Rc;

    use slint::platform::PointerEventButton;

    use crate::AppWindow;
    use crate::ui_harness::{Frame, Harness, SCALES};

    type Call = (i32, i32, f32, f32, bool, bool, bool);

    fn window(scale: f32) -> (Harness<AppWindow>, Rc<RefCell<Vec<Call>>>) {
        let h = Harness::new(scale, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        let calls: Rc<RefCell<Vec<Call>>> = Rc::default();
        let sink = calls.clone();
        h.comp.on_grid_pointer(move |k, b, x, y, c, s, a| {
            sink.borrow_mut().push((k, b, x, y, c, s, a));
        });
        h.advance(0);
        (h, calls)
    }

    #[test]
    fn down_up_and_move_are_reported_with_button_and_modifiers() {
        let (h, calls) = window(1.0);
        h.move_to(300.0, 200.0);
        h.press(300.0, 200.0, PointerEventButton::Left);
        h.move_to(320.0, 210.0);
        h.release(320.0, 210.0, PointerEventButton::Left);
        let c = calls.borrow().clone();
        assert!(c.iter().any(|x| x.0 == 0 && x.1 == 0), "{c:?}");
        assert!(c.iter().any(|x| x.0 == 2), "{c:?}");
        assert!(c.iter().any(|x| x.0 == 1 && x.1 == 0), "{c:?}");
        calls.borrow_mut().clear();
        h.press(300.0, 200.0, PointerEventButton::Right);
        h.release(300.0, 200.0, PointerEventButton::Right);
        assert!(calls.borrow().iter().any(|x| x.0 == 0 && x.1 == 2));
        calls.borrow_mut().clear();
        h.press(300.0, 200.0, PointerEventButton::Middle);
        h.release(300.0, 200.0, PointerEventButton::Middle);
        assert!(calls.borrow().iter().any(|x| x.0 == 0 && x.1 == 1));
    }

    #[test]
    fn positions_are_relative_to_the_grid_area() {
        for s in [1.0_f32, 1.5, 2.25] {
            let (h, calls) = window(s);
            h.press(100.0, 36.0 + 50.0, PointerEventButton::Left);
            let c = calls.borrow().clone();
            let d = c.iter().find(|x| x.0 == 0).expect("a press");
            assert!(
                (d.2 - (100.0 - h.comp.get_grid_left())).abs() <= 0.5,
                "scale {s}: {d:?}"
            );
            assert!((d.3 - 50.0).abs() <= 0.5, "scale {s}: {d:?}");
        }
    }

    #[test]
    fn a_drag_keeps_reporting_outside_the_area() {
        let (h, calls) = window(1.0);
        h.press(300.0, 200.0, PointerEventButton::Left);
        h.move_to(300.0, 6.0);
        h.release(300.0, 6.0, PointerEventButton::Left);
        let c = calls.borrow().clone();
        assert!(c.iter().any(|x| x.0 == 2 && x.3 < 0.0), "{c:?}");
        assert!(c.iter().any(|x| x.0 == 1), "{c:?}");
    }

    #[test]
    fn the_find_bar_is_inside_the_grid_area() {
        for s in SCALES {
            let (h, _) = window(s);
            h.comp.set_find_open(true);
            h.advance(0);
            let f = h.render();
            let red = [255, 0, 0, 255];
            let top = h.phys(h.comp.get_grid_top());
            let ink = |p: [u8; 4]| !Frame::near(p, [23, 23, 23, 255], 2) && !Frame::near(p, red, 2);
            let (_, y0, x1, _) = f
                .bbox_where(h.phys(500.0), top + 1, f.w, top + h.phys(60.0), ink)
                .unwrap_or_else(|| panic!("scale {s}: no bar"));
            let want = h.phys(12.0);
            assert!((f.w - (x1 + 1)).abs_diff(want) <= 1, "scale {s}: right gap");
            assert!((y0 - top).abs_diff(want) <= 1, "scale {s}: top gap");
        }
    }

    #[test]
    fn a_click_on_the_bar_does_not_reach_the_grid() {
        let (h, calls) = window(1.0);
        h.comp.set_find_open(true);
        h.advance(0);
        // Inside the bar (334 wide, 12 px from the right and from the grid top), off the controls.
        let x = 988.0 - 334.0 + 1.0;
        let y = 36.0 + 12.0 + 14.0;
        h.press(x, y, PointerEventButton::Left);
        h.release(x, y, PointerEventButton::Left);
        assert!(
            calls.borrow().iter().all(|c| c.0 != 0),
            "{:?}",
            calls.borrow()
        );
    }

    #[test]
    fn a_closed_bar_takes_no_pointer() {
        let (h, calls) = window(1.0);
        h.press(900.0, 36.0 + 26.0, PointerEventButton::Left);
        assert!(calls.borrow().iter().any(|c| c.0 == 0));
    }

    #[test]
    fn typing_in_the_bar_reaches_find_edited() {
        let (h, _) = window(1.0);
        let got: Rc<RefCell<String>> = Rc::default();
        let sink = got.clone();
        h.comp
            .on_find_edited(move |q| *sink.borrow_mut() = q.to_string());
        h.comp.set_find_open(true);
        h.advance(0);
        h.comp.invoke_focus_find();
        h.key_text("ab");
        assert_eq!(got.borrow().as_str(), "ab");
        assert!(h.comp.get_find_input_focused());
    }

    #[test]
    fn focus_terminal_takes_the_keys_back() {
        let (h, _) = window(1.0);
        h.comp.set_find_open(true);
        h.advance(0);
        h.comp.invoke_focus_find();
        h.comp.invoke_focus_terminal();
        h.key_text(&String::from(char::from(slint::platform::Key::F12)));
        assert!(h.comp.get_overlay_visible());
        assert!(!h.comp.get_find_input_focused());
    }

    #[test]
    fn the_badge_draws_over_the_grid() {
        let (h, _) = window(1.0);
        let before = h.render();
        h.comp.set_copied_phase(1);
        h.advance(500);
        let with = h.render();
        let (w, hh) = (with.w, with.h);
        let diff = |a: &Frame, b: &Frame| {
            let mut n = 0;
            for y in hh - 80..hh {
                for x in w / 2 - 100..w / 2 + 100 {
                    if a.px(x, y) != b.px(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(diff(&before, &with) > 50);
        h.comp.set_copied_phase(0);
        h.advance(500);
        assert_eq!(diff(&before, &h.render()), 0);
    }

    #[test]
    fn grid_origin_is_the_title_row_height() {
        for s in SCALES {
            let (h, _) = window(s);
            assert_eq!(h.comp.get_grid_top(), 36.0, "scale {s}");
            assert_eq!(h.comp.get_grid_left(), 0.0, "scale {s}");
        }
    }
}

// flows
// The input flows through the real window (Task 25): a `Harness<AppWindow>` plus an `AppCtl` over the
// fakes (fake pty, in-memory clipboard, recording openers, a settable clock), with the real grid host
// painting the live tab so a test reads pixels. Keys go through `AppCtl::on_key` first, the way the
// winit filter does; what the shell would get is read whole from the fake pty. Each test names the
// Prism Terminal e2e scenario it ports.

mod flow {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use flare_agent::keys::AgentView;
    use flare_agent::kinds::AgentKind;
    use flare_render::glyph::{FakeGlyphs, GlyphBitmap, GlyphKey, GlyphKind, GlyphSource};
    use flare_render::metrics::{BASE_FONT_PX, CellMetrics, FontUnits, cell_metrics_at};
    use flare_settings::kv::PrefStore;
    use flare_shells::list::ShellDef;
    use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, tabs_json};
    use flare_tabs::tabs_store::TabsStore;
    use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
    use flare_term::handle::TermHandle;
    use flare_term::keys::KeyEvent;
    use flare_win::clipboard_port::{ClipboardPort, MemoryClipboard};
    use flare_win::window_state::WindowStateStore;
    use slint::platform::PointerEventButton;
    use slint::{ComponentHandle, Model};

    use crate::AppWindow;
    use crate::appctl::{AppCtl, Deps, KeyOutcome, Post, TermPorts, UiMsg};
    use crate::host::{GridHost, GridPainter};
    use crate::live::LiveSource;
    use crate::openers::{OpenedRecord, RecordingOpeners};
    use crate::tab_host::TabHost;
    use crate::ui_harness::{Frame, Harness, SCALES};

    type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

    /// `Palette::pt_default().bg`, the link ink, the find fills and the selection blend over it.
    const GROUND: [u8; 4] = [23, 23, 23, 255];
    const LINK: [u8; 4] = [0x79, 0xa7, 0xd8, 255];
    const FIND_FILL: [u8; 4] = [0x4a, 0x51, 0x60, 255];
    const ACTIVE_FILL: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];
    const SELECTION: [u8; 4] = [78, 78, 78, 255];

    struct TestAgent(Cell<Option<AgentKind>>);
    impl AgentView for TestAgent {
        fn armed(&self, _tab: &str) -> Option<AgentKind> {
            self.0.get()
        }
    }

    /// A glyph source whose glyphs are solid boxes INSIDE their cell, so the fills and underlines the
    /// decoration paints around the text stay visible (`FakeGlyphs` fills the whole cell).
    struct BoxGlyphs {
        units: FontUnits,
        scale: f32,
        cache: HashMap<GlyphKey, GlyphBitmap>,
    }

    impl BoxGlyphs {
        fn new() -> Self {
            Self {
                units: FakeGlyphs::new(8, 16).font_units(),
                scale: 1.0,
                cache: HashMap::new(),
            }
        }
    }

    impl GlyphSource for BoxGlyphs {
        fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap> {
            if key.text.chars().all(|c| c == ' ') {
                return None;
            }
            if !self.cache.contains_key(key) {
                let m = cell_metrics_at(&self.units, BASE_FONT_PX, self.scale);
                let inset = (2.0 * self.scale).round().max(2.0) as u32;
                let width = m.cell_w.saturating_sub(2 * inset).max(1);
                let height = m.cell_h.saturating_sub(2 * inset).max(1);
                let bitmap = GlyphBitmap {
                    left: inset as i32,
                    top: m.baseline as i32 - inset as i32,
                    width,
                    height,
                    kind: GlyphKind::Mask,
                    data: vec![255u8; (width * height) as usize],
                };
                self.cache.insert(key.clone(), bitmap);
            }
            self.cache.get(key)
        }

        fn font_units(&self) -> FontUnits {
            self.units
        }

        fn set_scale(&mut self, scale: f32) {
            self.scale = scale;
            self.cache.clear();
        }
    }

    #[derive(Default, Clone)]
    struct Opts {
        /// The only shell the fake list offers (its id); `None`: the default PowerShell.
        shell: Option<&'static str>,
        /// Files made in the tab's folder; a trailing `/` makes a folder.
        files: Vec<&'static str>,
    }

    struct Wx {
        h: Harness<AppWindow>,
        ctl: Rc<RefCell<AppCtl>>,
        q: Queue,
        clock: Rc<Cell<u64>>,
        clip: Arc<MemoryClipboard>,
        openers: Arc<RecordingOpeners>,
        urls: Rc<RefCell<Vec<String>>>,
        agent: Rc<TestAgent>,
        host: Rc<RefCell<GridHost>>,
        handle: Arc<TermHandle>,
        fake: Arc<FakeState>,
        sp: Arc<FakeSpawner>,
        dir: tempfile::TempDir,
        prefs: PrefStore,
        scale: f32,
        /// The remote side of the spawner (remote sessions PR 3): a remote tab's link end.
        remote: Arc<flare_term::fake::FakeRemote>,
        /// The handle the grid shows now (the window's set_grid_source).
        slot: Rc<RefCell<Option<Arc<TermHandle>>>>,
    }

    fn ctrl(key: &str, code: &str) -> KeyEvent {
        KeyEvent::new(key, code).ctrl()
    }

    fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if cond() {
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        panic!("timed out waiting for {what}");
    }

    fn drain(q: &Queue, ctl: &Rc<RefCell<AppCtl>>) {
        loop {
            let m = q.lock().unwrap().pop_front();
            let Some(m) = m else { break };
            ctl.borrow_mut().on_msg(m);
        }
    }

    impl Wx {
        fn new(scale: f32) -> Wx {
            Self::with(scale, Opts::default())
        }

        fn with(scale: f32, opts: Opts) -> Wx {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().to_string_lossy().into_owned();
            // The tab's folder is a sub folder, so its spawn is told apart from any default tab in the home folder.
            let work = dir.path().join("work");
            std::fs::create_dir_all(&work).unwrap(); // sync-ok: test code only, never the app
            let work_s = work.to_string_lossy().into_owned();
            for f in &opts.files {
                let p = work.join(f.trim_end_matches('/'));
                if f.ends_with('/') {
                    std::fs::create_dir_all(&p).unwrap(); // sync-ok: test code only, never the app
                } else {
                    std::fs::write(&p, "x").unwrap(); // sync-ok: test code only, never the app
                }
            }
            let saved = SavedTabs {
                tabs: vec![SavedTab {
                    cwd: work_s.clone(),
                    agent: None,
                    remote: None,
                }],
                active: 0,
            };
            std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test code only, never the app
            let prefs = PrefStore::memory();
            let q: Queue = Arc::default();
            let post: Post = {
                let q = q.clone();
                Arc::new(move |m| q.lock().unwrap().push_back(m))
            };
            let sp = FakeSpawner::new();
            let waker: Arc<dyn Fn() + Send + Sync> = {
                let post = post.clone();
                Arc::new(move || post(UiMsg::HostWake))
            };
            let shells = match opts.shell {
                Some(id) => FakeShells::with(vec![ShellDef {
                    id: id.to_string(),
                    name: id.to_string(),
                    exe: format!("{id}.exe"),
                    args: Vec::new(),
                }]),
                None => FakeShells::new(),
            };
            let remote = flare_term::fake::FakeRemote::new();
            let tab_host = TabHost::new(
                Arc::new(flare_term::pty::RoutingSpawner {
                    local: sp.clone(),
                    remote: remote.clone(),
                }),
                shells,
                prefs.clone(),
                waker,
                None,
            );
            let clip = Arc::new(MemoryClipboard::new());
            let openers = Arc::new(RecordingOpeners::new());
            let agent = Rc::new(TestAgent(Cell::new(None)));
            let clock = Rc::new(Cell::new(0u64));
            let urls: Rc<RefCell<Vec<String>>> = Rc::default();
            let host = Rc::new(RefCell::new(GridHost::new(
                Box::new(BoxGlyphs::new()),
                GridPainter::Cpu,
            )));
            let slot: Rc<RefCell<Option<Arc<TermHandle>>>> = Rc::default();
            let inputs_cell: Rc<RefCell<Option<Rc<RefCell<crate::term_ui::DecorInputs>>>>> =
                Rc::default();
            let deps = Deps {
                prefs: prefs.clone(),
                tabs_store: Arc::new(TabsStore::new(
                    dir.path().join("tabs.json"),
                    Duration::from_millis(30),
                )),
                window_store: Arc::new(WindowStateStore::new(
                    dir.path().join("window.json"),
                    Duration::from_millis(30),
                )),
                host: tab_host,
                lookup: Arc::new(NoSessions),
                is_dir: Arc::new(|p| std::path::Path::new(p).is_dir()),
                home: home.clone(),
                clock: {
                    let c = clock.clone();
                    Rc::new(move || c.get())
                },
                post,
                pick_folder: Rc::new(|_, done| done(None)),
                clipboard: {
                    let c = clip.clone();
                    Rc::new(move |s| {
                        let _ = c.write_text(s, 4000);
                    })
                },
                open_url: {
                    let u = urls.clone();
                    Rc::new(move |s| u.borrow_mut().push(s.to_string()))
                },
                set_grid_source: {
                    let host = host.clone();
                    let slot = slot.clone();
                    let inputs_cell = inputs_cell.clone();
                    Rc::new(move |handle: Option<Arc<TermHandle>>| {
                        *slot.borrow_mut() = handle.clone();
                        match handle {
                            Some(h) => {
                                let source = match inputs_cell.borrow().clone() {
                                    Some(i) => LiveSource::with_inputs(h, i),
                                    None => LiveSource::new(h),
                                };
                                host.borrow_mut().set_source(Box::new(source));
                            }
                            None => host
                                .borrow_mut()
                                .set_source(Box::new(crate::host::SampleSource::new())),
                        }
                    })
                },
                raise: Rc::new(|| {}),
                quit: Rc::new(|| {}),
                hide: Rc::new(|| {}),
                starter_folders: Rc::new(|_| {}),
                launch_folders: Vec::new(),
                e2e: true,
                term: TermPorts {
                    clip_port: clip.clone(),
                    openers: openers.clone(),
                    agent: Some(agent.clone()),
                    dbl: flare_win::dblclick::FALLBACK,
                    metrics: {
                        let host = host.clone();
                        Rc::new(move || host.borrow().metrics())
                    },
                    mark_grid_dirty: {
                        let host = host.clone();
                        Rc::new(move || {
                            if let Ok(mut h) = host.try_borrow_mut() {
                                h.mark_dirty();
                            }
                        })
                    },
                },
            };
            let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
            let ctl = AppCtl::new(&h.comp, deps);
            // Pinned: the runner's Windows "show animations" switch must not decide the clock or the marks (the help panel and chrome motion still read the OS).
            ctl.borrow_mut().set_animations_source(Rc::new(|| true));
            *inputs_cell.borrow_mut() = Some(ctl.borrow().decor_inputs());
            // The window glue in miniature: the grid host sizes itself, paints, and tells the controller.
            {
                let host = host.clone();
                let weak = Rc::downgrade(&ctl);
                let app = h.comp.as_weak();
                h.comp.on_grid_resized(move |w, hgt, scale| {
                    host.borrow_mut()
                        .resize(w.max(0) as u32, hgt.max(0) as u32, scale);
                    let image = host.borrow_mut().render();
                    if let (Some(app), Some(img)) = (app.upgrade(), image) {
                        app.set_grid_image(img);
                    }
                    let (cols, rows) = host.borrow().dims();
                    if cols == 0 || rows == 0 {
                        return;
                    }
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.on_grid_dims(cols, rows);
                    }
                });
            }
            h.advance(0);
            // A change handler fires on a CHANGE after the first evaluation, so the first size is told by hand.
            h.comp.invoke_grid_resized(
                h.comp.get_grid_phys_width(),
                h.comp.get_grid_phys_height(),
                h.comp.get_scale(),
            );
            AppCtl::start_restore(&ctl);
            wait_for("the restore", || {
                drain(&q, &ctl);
                ctl.borrow().model().restore_done()
            });
            wait_for("the spawn", || {
                drain(&q, &ctl);
                sp.made().iter().any(|m| m.spec().cwd == work_s)
            });
            let handle = slot.borrow().clone().expect("a live tab in front");
            wait_for("the shell", || {
                drain(&q, &ctl);
                handle.shell_id().is_some()
            });
            let fake = sp
                .made()
                .into_iter()
                .find(|m| m.spec().cwd == work_s)
                .expect("the tab's pty");
            h.advance(0);
            let wx = Wx {
                h,
                ctl,
                q,
                clock,
                clip,
                openers,
                urls,
                agent,
                host,
                handle,
                fake,
                sp,
                dir,
                prefs: prefs.clone(),
                scale,
                remote,
                slot,
            };
            // The first render lays the window out, which tells the grid its size.
            let _ = wx.h.render();
            wx.pump();
            let _ = wx.frame();
            wx
        }

        // ---- plumbing -------------------------------------------------------------------------

        fn pump(&self) {
            drain(&self.q, &self.ctl);
            self.h.advance(0);
        }

        fn pump_until(&self, what: &str, mut cond: impl FnMut(&Wx) -> bool) {
            wait_for(what, || {
                self.pump();
                cond(self)
            });
        }

        /// The window clock and the Slint animation clock move together; the controller ticks.
        fn step(&self, ms: u64) {
            self.clock.set(self.clock.get() + ms);
            self.h.advance(ms);
            self.ctl.borrow_mut().tick();
            self.pump();
        }

        /// Ticks in 50 ms steps for `ms` (the timer's period).
        fn run(&self, ms: u64) {
            let mut left = ms;
            while left > 0 {
                let d = left.min(50);
                self.step(d);
                left -= d;
            }
        }

        /// The program prints (synchronously, no events).
        fn feed(&self, s: &str) {
            self.handle.with_emu(|e| {
                e.feed(s.as_bytes());
            });
            self.pump();
        }

        /// The program prints through the pty reader, so its events (OSC 52, cwd reports) arrive too;
        /// returns once `cond` holds.
        fn print(&self, s: &str, mut cond: impl FnMut(&Wx) -> bool) {
            self.fake.feed(s.as_bytes());
            self.pump_until("the output", |w| cond(w));
        }

        fn written(&self) -> Vec<u8> {
            self.fake.written()
        }

        fn wait_written(&self, want: &[u8]) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(5) && self.fake.written().len() < want.len() {
                self.pump();
                std::thread::sleep(Duration::from_millis(3));
            }
            let all: Vec<_> = self.sp.made().iter().map(|m| m.written()).collect();
            assert_eq!(self.fake.written(), want, "all ptys: {all:?}");
        }

        /// Nothing more reaches the pty in the next 80 ms.
        fn quiet(&self) -> Vec<u8> {
            std::thread::sleep(Duration::from_millis(80));
            self.pump();
            self.fake.written()
        }

        fn text(&self) -> String {
            self.handle.with_emu_quiet(|e| e.sel_text())
        }

        fn has_sel(&self) -> bool {
            self.handle.with_emu_quiet(|e| e.has_selection())
        }

        // ---- geometry, from the live layout -----------------------------------------------------

        fn metrics(&self) -> CellMetrics {
            self.host.borrow().metrics()
        }

        fn grid_origin_css(&self) -> (f32, f32) {
            (self.h.comp.get_grid_left(), self.h.comp.get_grid_top())
        }

        /// The window css position `frac` (0..1) across cell (`col`, `row`), half way down it.
        fn cell_at(&self, col: usize, row: usize, frac: f32) -> (f32, f32) {
            let m = self.metrics();
            let (gl, gt) = self.grid_origin_css();
            let s = self.scale;
            let x = gl + (m.padding as f32 + (col as f32 + frac) * m.cell_w as f32) / s;
            let y =
                gt + (m.padding as f32 + row as f32 * m.cell_h as f32 + m.cell_h as f32 / 2.0) / s;
            (x, y)
        }

        /// The css centre of a cell: `grid_left + (pad + col * cell_w + cell_w / 2) / scale`.
        fn cell_center_css(&self, col: usize, row: usize) -> (f32, f32) {
            self.cell_at(col, row, 0.5)
        }

        /// The cell's rectangle in the rendered frame (device px): x, y, w, h.
        fn cell_rect(&self, col: usize, row: usize) -> (u32, u32, u32, u32) {
            let m = self.metrics();
            let (gl, gt) = self.grid_origin_css();
            let x = self.h.phys(gl) + m.padding + col as u32 * m.cell_w;
            let y = self.h.phys(gt) + m.padding + row as u32 * m.cell_h;
            (x, y, m.cell_w, m.cell_h)
        }

        /// Cell (`col`, `row`)'s pixel at an offset inside it.
        fn cell_px(&self, f: &Frame, col: usize, row: usize, dx: u32, dy: u32) -> [u8; 4] {
            let (x, y, _, _) = self.cell_rect(col, row);
            f.px(x + dx, y + dy)
        }

        /// Pixels of exactly `want` inside the cells `cols` of `row`.
        fn count_in(
            &self,
            f: &Frame,
            row: usize,
            cols: std::ops::Range<usize>,
            want: [u8; 4],
        ) -> usize {
            let mut n = 0;
            for c in cols {
                let (x, y, w, h) = self.cell_rect(c, row);
                n += f.count_where(x, y, x + w, y + h, |p| p == want);
            }
            n
        }

        /// Paints the grid when it changed, then the window.
        fn frame(&self) -> Frame {
            self.pump();
            self.paint_grid();
            self.h.advance(0);
            self.h.render()
        }

        /// The grid host's own paint (layout, link scan, decoration, cells) when it changed: how long it took.
        fn paint_grid(&self) -> Duration {
            let t = Instant::now();
            if self.host.borrow().is_dirty() {
                let image = self.host.borrow_mut().render();
                if let Some(img) = image {
                    self.h.comp.set_grid_image(img);
                }
            }
            t.elapsed()
        }

        // ---- the pointer ----------------------------------------------------------------------

        fn to(&self, at: (f32, f32)) {
            self.h.move_to(at.0, at.1);
        }

        fn down(&self, at: (f32, f32), b: PointerEventButton) {
            self.h.move_to(at.0, at.1);
            self.h.press(at.0, at.1, b);
        }

        fn up(&self, at: (f32, f32), b: PointerEventButton) {
            self.h.release(at.0, at.1, b);
            self.pump();
        }

        /// A press and release of the left button inside the cell's left half (a click on the exact middle is
        /// on the boundary the caret rule rounds).
        fn click(&self, col: usize, row: usize) {
            let p = self.cell_at(col, row, 0.25);
            self.down(p, PointerEventButton::Left);
            self.up(p, PointerEventButton::Left);
        }

        fn right_click(&self, col: usize, row: usize) {
            let p = self.cell_at(col, row, 0.25);
            self.down(p, PointerEventButton::Right);
            self.up(p, PointerEventButton::Right);
        }

        /// A drag over the boundaries `a..b` of `row` (the left quarter of each cell).
        fn drag(&self, row: usize, a: usize, b: usize) {
            let from = self.cell_at(a, row, 0.25);
            let to = self.cell_at(b, row, 0.25);
            self.down(from, PointerEventButton::Left);
            self.to(to);
            self.up(to, PointerEventButton::Left);
        }

        /// A raw grid-pointer event (kind 0 down, 1 up, 2 move) with modifiers: the harness has none.
        fn raw(&self, kind: i32, button: i32, at: (f32, f32), ctrl: bool, shift: bool, alt: bool) {
            let (gl, gt) = self.grid_origin_css();
            self.h
                .comp
                .invoke_grid_pointer(kind, button, at.0 - gl, at.1 - gt, ctrl, shift, alt);
            self.pump();
        }

        fn raw_drag(&self, row: usize, a: usize, b: usize, shift: bool, alt: bool) {
            let from = self.cell_at(a, row, 0.25);
            let to = self.cell_at(b, row, 0.25);
            self.raw(0, 0, from, false, shift, alt);
            self.raw(2, 0, to, false, shift, alt);
            self.raw(1, 0, to, false, shift, alt);
        }

        // ---- keys -----------------------------------------------------------------------------

        /// A key the way the window filter treats it: the controller first; what it leaves for the
        /// shell is typed, what it leaves for a Slint control is dispatched to the window.
        fn key(&self, ev: KeyEvent) -> KeyOutcome {
            let out = self.ctl.borrow_mut().on_key(&ev);
            match out {
                KeyOutcome::Pty => {
                    self.handle.send_key(&ev);
                }
                KeyOutcome::Slint => {
                    if let Some(t) = &ev.text {
                        if ev.shift || ev.ctrl {
                            self.h.key_with(ev.shift, ev.ctrl, t);
                        } else {
                            self.h.key_text(t);
                        }
                    }
                }
                KeyOutcome::Consumed => {}
            }
            self.pump();
            out
        }

        /// Types plain text one character at a time through the controller.
        fn type_text(&self, text: &str) {
            for ch in text.chars() {
                let s = ch.to_string();
                self.key(KeyEvent::new(&s, "").text(&s));
            }
        }

        fn ctrl_c(&self) -> KeyOutcome {
            self.key(ctrl("c", "KeyC"))
        }

        /// A key typed into the tab the grid shows now (the window glue's way), not the first tab's.
        fn key_front(&self, ev: KeyEvent) {
            if self.ctl.borrow_mut().on_key(&ev) == KeyOutcome::Pty {
                let front = self.slot.borrow().clone().expect("a tab in front");
                front.send_key(&ev);
            }
            self.pump();
        }

        // ---- reads ----------------------------------------------------------------------------

        fn records(&self) -> Vec<OpenedRecord> {
            self.openers.records()
        }

        fn clip_text(&self) -> Option<String> {
            self.clip.read_text()
        }

        /// The rows of the terminal menu except the last, Command help (M7 Task 43): that row is
        /// pinned by the help tests (`help_ctl`, `appctl`), these assert the M5 rows around it.
        fn menu_labels(&self) -> Vec<String> {
            self.h
                .comp
                .get_menu_rows()
                .iter()
                .map(|r| r.label.to_string())
                .filter(|l| l != "Command help")
                .collect()
        }
    }

    // ---- selection ----------------------------------------------------------------------------

    /// PT selectionEdit: a drag selects, Ctrl+C copies and the pill says Copied, then fades.
    #[test]
    fn a_drag_then_ctrl_c_copies_and_says_copied() {
        let w = Wx::new(1.0);
        w.feed("hello world");
        let before = w.frame();
        w.drag(0, 0, 5);
        assert_eq!(w.text(), "hello");
        let f = w.frame();
        // The selection blend shows around the glyph boxes of the five cells.
        assert!(w.count_in(&f, 0, 0..5, SELECTION) > 100);
        assert_eq!(w.count_in(&f, 0, 5..11, SELECTION), 0);
        assert_eq!(w.ctrl_c(), KeyOutcome::Consumed);
        w.pump_until("the copy", |w| w.clip_text().as_deref() == Some("hello"));
        assert!(!w.has_sel());
        w.pump_until("the badge", |w| w.h.comp.get_copied_phase() == 1);
        w.step(50);
        let pill = w.frame();
        let diff = |a: &Frame, b: &Frame| {
            let (x0, x1) = (a.w / 2 - 100, a.w / 2 + 100);
            let mut n = 0;
            for y in a.h - 80..a.h {
                for x in x0..x1 {
                    if a.px(x, y) != b.px(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(
            diff(&before, &pill) > 50,
            "the pill is drawn at the bottom centre"
        );
        w.run(1200);
        assert_eq!(w.h.comp.get_copied_phase(), 2);
        w.run(200);
        w.step(50);
        assert_eq!(w.h.comp.get_copied_phase(), 0);
        let gone = w.frame();
        assert_eq!(diff(&before, &gone), 0, "no pill pixels remain");
    }

    impl Wx {
        fn hair_phys(&self) -> u32 {
            self.scale.floor().max(1.0) as u32
        }

        /// The hairline in css px, as `Px.hair()` yields it.
        fn hair_css(&self) -> f32 {
            self.hair_phys() as f32 / self.scale
        }

        /// Clicks the open menu's row with this label, the way a person does.
        fn pick(&self, label: &str) {
            let labels = self.menu_labels();
            let i = labels
                .iter()
                .position(|l| l == label)
                .unwrap_or_else(|| panic!("no menu row {label:?} in {labels:?}"));
            let hair = self.hair_css();
            let x = self.h.comp.get_menu_x() + 40.0;
            let y = self.h.comp.get_menu_y() + hair + i as f32 * (28.0 + hair) + 14.0;
            self.h.click(x, y);
            self.pump();
        }

        fn escape(&self) -> KeyOutcome {
            self.key(KeyEvent::new("Escape", "Escape").text("\u{1b}"))
        }

        fn backspace(&self) -> KeyOutcome {
            self.key(KeyEvent::new("Backspace", "Backspace"))
        }

        /// Frames and ticks until the link marks of the viewport have settled.
        fn settle(&self) -> Frame {
            let mut f = self.frame();
            for _ in 0..4 {
                self.step(50);
                f = self.frame();
            }
            f
        }
    }

    const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1006h";

    /// PT scenario selectionEdit.
    #[test]
    fn double_and_triple_click_select_word_and_line() {
        let w = Wx::new(1.0);
        w.feed("hello world foo");
        w.click(2, 0);
        w.step(100);
        w.click(2, 0);
        assert_eq!(w.text(), "hello");
        assert_eq!(w.ctrl_c(), KeyOutcome::Consumed);
        w.pump_until("the word", |w| w.clip_text().as_deref() == Some("hello"));
        w.step(1000);
        w.click(2, 0);
        w.step(100);
        w.click(2, 0);
        w.step(100);
        w.click(2, 0);
        assert_eq!(w.text(), "hello world foo");
        assert_eq!(w.ctrl_c(), KeyOutcome::Consumed);
        w.pump_until("the line", |w| {
            w.clip_text().as_deref() == Some("hello world foo")
        });
    }

    /// PT scenario selectionEdit.
    #[test]
    fn shift_click_extends_and_alt_drag_is_a_block() {
        let w = Wx::new(1.0);
        w.feed("abcdefghij\r\nklmnopqrst\r\nuvwxyz");
        w.drag(0, 2, 4);
        assert_eq!(w.text(), "cd");
        let at = w.cell_at(7, 0, 0.25);
        w.raw(0, 0, at, false, true, false);
        w.raw(1, 0, at, false, true, false);
        assert_eq!(w.text(), "cdefg", "shift extends the end");
        // A fresh block with Alt: the same columns of three rows.
        w.click(15, 8);
        assert!(!w.has_sel());
        let from = w.cell_at(1, 0, 0.25);
        let to = w.cell_at(4, 2, 0.25);
        w.raw(0, 0, from, false, false, true);
        w.raw(2, 0, to, false, false, true);
        w.raw(1, 0, to, false, false, true);
        assert_eq!(w.text().replace("\r\n", "\n"), "bcd\nlmn\nvwx");
        let f = w.frame();
        assert!(
            w.count_in(&f, 1, 1..4, SELECTION) > 50,
            "the block covers the middle row"
        );
        assert_eq!(
            w.count_in(&f, 1, 4..8, SELECTION),
            0,
            "and stops at its right edge"
        );
    }

    /// PT scenario selectionEdit.
    #[test]
    fn backspace_deletes_the_selection_on_the_edited_line() {
        let w = Wx::new(1.0);
        w.feed("echo hello");
        w.drag(0, 7, 10);
        assert_eq!(w.text(), "llo");
        assert_eq!(w.backspace(), KeyOutcome::Consumed);
        w.wait_written(b"\x7f\x7f\x7f");
        assert!(!w.has_sel());
        assert!(w.handle.last_typed().is_some());
        // A selection on older output is not the edited line: Backspace is one plain key.
        let w = Wx::new(1.0);
        w.feed("older line\r\nPS> abc");
        w.drag(0, 0, 5);
        assert_eq!(w.backspace(), KeyOutcome::Pty);
        w.wait_written(b"\x7f");
        assert_eq!(w.quiet(), b"\x7f", "once");
        assert!(w.has_sel());
    }

    /// PT scenario selectionEdit.
    #[test]
    fn the_menu_fits_what_was_clicked_and_copies_exactly() {
        let w = Wx::new(1.0);
        let url = format!("https://example.com/{}", "a".repeat(150));
        w.feed(&format!("plain text here\r\nsee marked words\r\n{url} end"));
        w.settle();
        // Plain text: Paste and Find, no Close tab.
        w.right_click(3, 0);
        assert!(w.h.comp.get_menu_open());
        assert_eq!(w.menu_labels(), ["Paste", "Find in scrollback"]);
        // Every row wears a glyph: ink inside the 13 px icon box left of the label.
        let f = w.frame();
        let hair = w.hair_phys();
        let (mx, my) = (
            w.h.phys(w.h.comp.get_menu_x()),
            w.h.phys(w.h.comp.get_menu_y()),
        );
        for i in 0..2u32 {
            let x0 = mx + hair + w.h.phys(11.0);
            let y0 = my + hair + i * (w.h.phys(28.0) + hair) + w.h.phys(7.0);
            let side_flat = [31, 31, 30, 255];
            let ink = f.count_where(x0, y0, x0 + w.h.phys(13.0), y0 + w.h.phys(14.0), |p| {
                !Frame::near(p, side_flat, 6)
            });
            assert!(ink > 5, "row {i} shows no glyph");
        }
        w.escape();
        assert!(!w.h.comp.get_menu_open());
        // Marked text: Copy comes first.
        w.drag(1, 4, 10);
        assert_eq!(w.text(), "marked");
        w.right_click(6, 1);
        assert_eq!(w.menu_labels(), ["Copy", "Paste", "Find in scrollback"]);
        w.escape();
        w.click(15, 10);
        assert!(!w.has_sel());
        // A url that wrapped over two rows: the whole of it is copied, from either row.
        w.right_click(5, 2);
        assert_eq!(
            w.menu_labels(),
            ["Copy link", "Open link", "Paste", "Find in scrollback"]
        );
        w.pick("Copy link");
        w.pump_until("the link copy", |w| {
            w.clip_text().as_deref() == Some(url.as_str())
        });
        w.right_click(5, 3);
        assert_eq!(w.menu_labels()[0], "Copy link");
        w.pick("Open link");
        assert_eq!(*w.urls.borrow(), vec![url.clone()], "opened once");
        assert!(!w.h.comp.get_menu_open());
    }

    /// Owner, 2026-10-23: "right click should open the right click menu each time, now it alternates
    /// one click opens it the next closes it". With a menu open, a right click outside it closes it
    /// AND opens the menu of whatever is under the pointer, in one click; a left press only closes.
    #[test]
    fn a_right_click_outside_an_open_menu_reopens_it_for_what_is_under_the_pointer() {
        for scale in [1.0_f32, 2.25] {
            let w = Wx::new(scale);
            w.feed("plain text here\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\nsee https://example.com/x end");
            w.settle();
            w.right_click(3, 0);
            assert_eq!(
                w.menu_labels(),
                ["Paste", "Find in scrollback"],
                "scale {scale}"
            );
            // Right click on the link: the same click closes the plain menu and opens the link's.
            let link = w.cell_at(8, 8, 0.25);
            w.h.right_click(link.0, link.1);
            w.pump();
            assert!(w.h.comp.get_menu_open(), "scale {scale}");
            assert_eq!(
                w.menu_labels()[0],
                "Copy link",
                "scale {scale}: the link's rows"
            );
            assert!(
                (w.h.comp.get_menu_x() - link.0).abs() < 1.0
                    && (w.h.comp.get_menu_y() - link.1).abs() < 1.0,
                "scale {scale}: hung at the new point"
            );
            // And back on plain text: the plain rows again (each click opens, none toggles shut).
            let plain = w.cell_at(14, 0, 0.25);
            w.h.right_click(plain.0, plain.1);
            w.pump();
            assert!(w.h.comp.get_menu_open(), "scale {scale}");
            assert_eq!(
                w.menu_labels(),
                ["Paste", "Find in scrollback"],
                "scale {scale}"
            );
            // A left press outside only closes: nothing opens, nothing is picked.
            let away = w.cell_at(20, 5, 0.25);
            w.h.click(away.0, away.1);
            w.pump();
            assert!(
                !w.h.comp.get_menu_open(),
                "scale {scale}: a left press only closes"
            );
            // Right clicks keep working one after the other with no menu in between.
            for _ in 0..3 {
                w.h.right_click(plain.0, plain.1);
                w.pump();
                assert!(w.h.comp.get_menu_open(), "scale {scale}");
            }
        }
    }

    /// The same right click at the same place reopens the menu every time: in the open, where the panel
    /// hangs from the spot, and at the bottom edge, where the panel is pushed up over the spot.
    /// A link written under the spot while the menu stands proves it is rebuilt, not left as it was.
    #[test]
    fn a_right_click_on_the_same_spot_reopens_it_every_time() {
        for scale in [1.0_f32, 2.25] {
            let w = Wx::new(scale);
            w.feed("plain text here");
            w.settle();
            for (col, row, put) in [
                (3, 0, "\x1b[1;1Hhttps://example.com/x"),
                (3, 36, "\x1b[37;1Hhttps://example.com/x"),
            ] {
                let spot = w.cell_at(col, row, 0.25);
                w.h.right_click(spot.0, spot.1);
                w.pump();
                assert!(w.h.comp.get_menu_open(), "scale {scale}");
                assert_eq!(w.menu_labels()[0], "Paste", "scale {scale}: plain first");
                let first = (w.h.comp.get_menu_x(), w.h.comp.get_menu_y());
                w.feed(put);
                w.settle();
                w.h.right_click(spot.0, spot.1);
                w.pump();
                assert!(
                    w.h.comp.get_menu_open(),
                    "scale {scale}: never toggled shut"
                );
                assert_eq!(
                    w.menu_labels()[0],
                    "Copy link",
                    "scale {scale} col {col}: rebuilt for what is under the spot now"
                );
                assert_eq!(first, (w.h.comp.get_menu_x(), w.h.comp.get_menu_y()));
                w.escape();
            }
        }
    }

    /// A right click inside the open menu keeps its old meaning (nothing happens, the menu stays), and
    /// a program that owns the mouse still gets the outside right click it would get without a menu.
    #[test]
    fn an_outside_right_click_goes_where_a_normal_one_would() {
        let w = Wx::new(1.0);
        // A right click is reported to a mouse program only when the owner switched that on (default off).
        flare_settings::term_prefs::set_right_click_reports(&w.prefs, true);
        w.feed("hello world");
        w.settle();
        w.right_click(2, 0);
        assert!(w.h.comp.get_menu_open());
        let inside = (w.h.comp.get_menu_x() + 40.0, w.h.comp.get_menu_y() + 14.0);
        w.h.right_click(inside.0, inside.1);
        w.pump();
        assert!(w.h.comp.get_menu_open(), "inside the menu: unchanged");
        // The program turns the mouse on; an outside right click closes the menu and is reported.
        w.feed(MOUSE_ON);
        let at = w.cell_center_css(10, 6);
        w.h.right_click(at.0, at.1);
        w.pump();
        // A right click over a mouse program is reported and still offers the menu; the outside one does
        // exactly that, once, and the old menu is gone.
        assert!(w.h.comp.get_menu_open(), "the new menu");
        assert!(
            (w.h.comp.get_menu_x() - at.0).abs() < 1.0,
            "hung at the new point"
        );
        w.wait_written(b"\x1b[<2;11;7M\x1b[<2;11;7m");
        // Once: more turns of the loop bring no second report.
        for _ in 0..5 {
            w.pump();
        }
        assert_eq!(
            w.fake.written(),
            b"\x1b[<2;11;7M\x1b[<2;11;7m",
            "reported once"
        );
    }

    /// PT scenario selectionEdit.
    #[test]
    fn ctrl_c_over_a_selection_is_not_the_interrupt_and_without_one_it_is() {
        let w = Wx::new(1.0);
        w.feed("hello world");
        w.drag(0, 0, 5);
        assert_eq!(w.ctrl_c(), KeyOutcome::Consumed);
        w.pump_until("the copy", |w| w.clip_text().as_deref() == Some("hello"));
        assert_eq!(w.quiet(), b"", "a copy sends nothing to the shell");
        assert_eq!(w.ctrl_c(), KeyOutcome::Pty);
        w.wait_written(b"\x03");
    }

    /// PT reviewKeys: the Russian layout types another letter, the physical key is still C.
    #[test]
    fn ctrl_c_by_physical_key_copies() {
        let w = Wx::new(1.0);
        w.feed("hello world");
        w.drag(0, 6, 11);
        assert_eq!(w.key(ctrl("\u{441}", "KeyC")), KeyOutcome::Consumed);
        w.pump_until("the copy", |w| w.clip_text().as_deref() == Some("world"));
        assert_eq!(w.quiet(), b"");
    }

    /// PT scenario clickCaret.
    #[test]
    fn a_click_on_the_typed_line_moves_the_caret() {
        let w = Wx::new(1.0);
        w.feed("old output\r\nPS> echo abc");
        // Older output: a click does nothing.
        w.click(3, 0);
        assert_eq!(w.quiet(), b"");
        // The line being typed: two cells left of the end is two arrows.
        w.click(10, 1);
        w.wait_written(b"\x1b[D\x1b[D");
        assert!(w.handle.last_typed().is_some());
    }

    /// xterm `altClickMovesCursor`, left on by Prism Terminal (owner question 5, answered "include").
    #[test]
    fn an_alt_click_moves_the_cursor_with_arrows_and_selects_nothing() {
        let w = Wx::new(1.0);
        w.feed("old output\r\nPS> echo abc");
        // The cursor is at column 12 of row 1; an Alt+click on column 3 is nine lefts.
        let at = w.cell_at(3, 1, 0.25);
        w.raw(0, 0, at, false, false, true);
        w.raw(1, 0, at, false, false, true);
        w.wait_written(&b"\x1b[D".repeat(9));
        assert!(!w.has_sel());
        assert!(
            w.handle.last_typed().is_some(),
            "an Alt+click is the user's act"
        );
    }

    /// PT scenario mouseReport.
    #[test]
    fn a_mouse_program_gets_reports_and_shift_selects() {
        let w = Wx::new(1.0);
        w.feed("hello world");
        w.feed(MOUSE_ON);
        let at = w.cell_center_css(3, 1);
        w.down(at, PointerEventButton::Left);
        w.up(at, PointerEventButton::Left);
        w.wait_written(b"\x1b[<0;4;2M\x1b[<0;4;2m");
        assert!(!w.has_sel());
        assert!(w.handle.last_typed().is_none(), "a report is not typing");
        // Shift gives the mouse back to the terminal: a selection and no report.
        let n = w.written().len();
        w.raw_drag(0, 0, 5, true, false);
        assert_eq!(w.text(), "hello");
        assert_eq!(w.quiet().len(), n);
        assert!(w.handle.last_typed().is_none());
    }

    impl Wx {
        fn work(&self) -> std::path::PathBuf {
            self.dir.path().join("work")
        }

        fn cols(&self) -> usize {
            self.handle.with_emu_quiet(|e| e.cols())
        }

        /// Steps and paints until `cond` holds on the frame (a link scan or a stat worker answered).
        fn frame_until(&self, what: &str, mut cond: impl FnMut(&Wx, &Frame) -> bool) -> Frame {
            let t = Instant::now();
            loop {
                self.step(50);
                let f = self.frame();
                if cond(self, &f) {
                    return f;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(10),
                    "timed out waiting for {what}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// Like `count_in`, inside each cell's one pixel margin: the outline a match draws OUTSIDE its
        /// box is left out.
        fn fill_in(
            &self,
            f: &Frame,
            row: usize,
            cols: std::ops::Range<usize>,
            want: [u8; 4],
        ) -> usize {
            let mut n = 0;
            for c in cols {
                let (x, y, w, h) = self.cell_rect(c, row);
                n += f.count_where(x + 1, y + 1, x + w - 1, y + h - 1, |p| p == want);
            }
            n
        }

        /// Every pixel of the cells `cols` of `row` that is exactly the link ink.
        fn inked(&self, f: &Frame, row: usize, cols: std::ops::Range<usize>) -> usize {
            self.count_in(f, row, cols, LINK)
        }
    }

    /// The top-right strip of the grid, where the find bar sits (device px: x0, y0, x1, y1).
    fn bar_region(w: &Wx, f: &Frame) -> (u32, u32, u32, u32) {
        let top = w.h.phys(w.h.comp.get_grid_top());
        (f.w - w.h.phys(360.0), top, f.w, top + w.h.phys(60.0))
    }

    /// The box of the pixels that differ between two frames inside `region`.
    fn diff_box(
        a: &Frame,
        b: &Frame,
        region: (u32, u32, u32, u32),
    ) -> Option<(u32, u32, u32, u32)> {
        let mut out: Option<(u32, u32, u32, u32)> = None;
        for y in region.1..region.3.min(a.h) {
            for x in region.0..region.2.min(a.w) {
                if a.px(x, y) != b.px(x, y) {
                    out = Some(match out {
                        None => (x, y, x, y),
                        Some((p, q, r, s)) => (p.min(x), q.min(y), r.max(x), s.max(y)),
                    });
                }
            }
        }
        out
    }

    // ---- links --------------------------------------------------------------------------------

    /// PT links: the url is inked and underlined, its trailing full stop is not, and a url that wrapped
    /// over two rows is inked on both.
    #[test]
    fn a_printed_link_wears_the_link_ink_and_its_full_stop_does_not() {
        let w = Wx::new(1.0);
        w.feed("see https://example.com/page. done");
        let f = w.settle();
        for col in 4..28 {
            assert!(w.inked(&f, 0, col..col + 1) > 0, "cell {col} is not inked");
        }
        assert_eq!(
            w.inked(&f, 0, 28..29),
            0,
            "the full stop is not part of the link"
        );
        assert_eq!(w.inked(&f, 0, 0..4), 0);
        assert_eq!(w.inked(&f, 0, 29..34), 0);
        // The underline: the cell's bottom row is the faint link ink, and absent under the full stop.
        let (_, _, _, ch) = w.cell_rect(10, 0);
        assert!(
            !Frame::near(w.cell_px(&f, 10, 0, 1, ch - 1), GROUND, 2),
            "an underline row exists"
        );
        assert!(Frame::near(w.cell_px(&f, 28, 0, 1, ch - 1), GROUND, 2));
        // A url that wraps: ink on the end of the first row and the start of the second.
        let w = Wx::new(1.0);
        let cols = w.cols();
        let url = format!("https://example.com/{}", "b".repeat(30));
        let lead = cols - 12;
        w.feed(&format!("{} {url}", "x".repeat(lead - 1)));
        let f = w.settle();
        assert!(w.inked(&f, 0, lead..cols) > 100, "the first row is inked");
        let rest = url.len() - (cols - lead);
        assert!(w.inked(&f, 1, 0..rest) > 100, "the second row is inked");
        assert_eq!(w.inked(&f, 1, rest..rest + 4), 0);
    }

    /// PT scenario links.
    #[test]
    fn an_alternate_screen_link_is_inked() {
        let w = Wx::new(1.0);
        w.feed("\x1b[?1049hsee https://example.com/x now");
        assert!(w.handle.mode().alt_screen);
        let f = w.settle();
        for col in 4..25 {
            assert!(w.inked(&f, 0, col..col + 1) > 0, "cell {col} is not inked");
        }
        assert_eq!(w.inked(&f, 0, 25..32), 0);
    }

    /// PT scenario osc8Links.
    #[test]
    fn an_osc8_label_wears_the_ink_and_opens_once_on_a_left_click() {
        let w = Wx::new(1.0);
        w.feed(
            "\x1b]8;;https://example.com/osc\x07LABEL\x1b]8;;\x07 and \x1b]8;;file:///C:/x.txt\x07FILE\x1b]8;;\x07",
        );
        let f = w.settle();
        for col in 0..5 {
            assert!(w.inked(&f, 0, col..col + 1) > 0, "label cell {col}");
        }
        assert_eq!(w.inked(&f, 0, 10..14), 0, "a file:/// label is not a link");
        w.click(2, 0);
        assert_eq!(*w.urls.borrow(), ["https://example.com/osc"], "one open");
        w.right_click(2, 0);
        assert_eq!(w.urls.borrow().len(), 1, "a right click opens nothing");
        assert!(w.h.comp.get_menu_open(), "and offers the menu");
    }

    /// Drives the frames until `cols` of row 0 wear the link ink.
    fn light_paths(w: &Wx, cols: std::ops::Range<usize>) -> Frame {
        w.frame_until("the file links", |w, f| w.inked(f, 0, cols.clone()) > 0)
    }

    /// Spec 2026-11-11 (replaces PT scenario pathLinks): a path is a file link only between the marks a session
    /// writes, whether or not it exists, and a click opens it by the allowlist.
    #[test]
    fn a_path_is_a_link_only_between_marks_and_opens_by_the_allowlist() {
        let w = Wx::with(
            1.0,
            Opts {
                files: vec!["a.pdf", "b.bat", "missing.txt", "folder/"],
                ..Opts::default()
            },
        );
        // [a.pdf] 0..7, [b.bat] 8..15, missing.txt 16..27 (exists, but no marks), [folder/] 28..37
        w.feed("\u{27E6}a.pdf\u{27E7} \u{27E6}b.bat\u{27E7} missing.txt \u{27E6}folder/\u{27E7}");
        let f = light_paths(&w, 1..6);
        assert!(w.inked(&f, 0, 9..14) > 0, "b.bat is marked");
        assert!(w.inked(&f, 0, 29..36) > 0, "a folder is a link too");
        assert_eq!(
            w.inked(&f, 0, 16..27),
            0,
            "a path with no marks is never guessed"
        );
        let a = w.work().join("a.pdf").to_string_lossy().into_owned();
        let b = w.work().join("b.bat").to_string_lossy().into_owned();
        w.click(2, 0);
        w.pump_until("the open", |w| !w.records().is_empty());
        assert_eq!(w.records(), [OpenedRecord::Open(a.clone())]);
        // A script is never run by a click: revealed, and never opened (O22).
        w.click(10, 0);
        w.pump_until("the reveal", |w| w.records().len() == 2);
        assert_eq!(w.records()[1], OpenedRecord::Reveal(b.clone()));
        assert!(!w.records().contains(&OpenedRecord::Open(b)));
        // The menu on a file link: open, show, copy the whole absolute path.
        w.right_click(2, 0);
        assert_eq!(
            w.menu_labels(),
            [
                "Open",
                flare_win::openers::REVEAL_LABEL,
                "Copy path",
                "Paste",
                "Find in scrollback"
            ]
        );
        w.pick("Copy path");
        w.pump_until("the path copy", |w| {
            w.clip_text().as_deref() == Some(a.as_str())
        });
    }

    /// Spec 2026-11-11 (replaces PT scenario pathLinks): the menu on a file link goes through the allowlist.
    #[test]
    fn a_path_menu_open_and_show_go_through_the_allowlist() {
        let w = Wx::with(
            1.0,
            Opts {
                files: vec!["a.txt", "b.bat"],
                ..Opts::default()
            },
        );
        // [a.txt] 0..7, [b.bat] 8..15
        w.feed("\u{27E6}a.txt\u{27E7} \u{27E6}b.bat\u{27E7}");
        light_paths(&w, 1..6);
        let a = w.work().join("a.txt").to_string_lossy().into_owned();
        let b = w.work().join("b.bat").to_string_lossy().into_owned();
        w.right_click(10, 0);
        w.pick("Open");
        w.pump_until("the reveal", |w| !w.records().is_empty());
        assert_eq!(
            w.records(),
            [OpenedRecord::Reveal(b.clone())],
            "Open on a script only reveals"
        );
        w.right_click(2, 0);
        w.pick(flare_win::openers::REVEAL_LABEL);
        w.pump_until("the show", |w| w.records().len() == 2);
        assert_eq!(w.records()[1], OpenedRecord::Reveal(a.clone()));
        assert!(
            !w.records()
                .iter()
                .any(|r| matches!(r, OpenedRecord::Open(_)))
        );
        w.right_click(2, 0);
        w.pick("Copy path");
        w.pump_until("the path copy", |w| {
            w.clip_text().as_deref() == Some(a.as_str())
        });
    }

    /// Owner, 2026-11-11: on a table link that names nothing, Open said nothing and Copy path copied the path read
    /// across the borders. Open now says "Flare found no such file" and calls no opener; Copy path copies exactly the
    /// path in the cell, as written (rules 273 and 274).
    #[test]
    fn a_table_cell_link_that_names_nothing_opens_nothing_and_copies_exactly_its_path() {
        // The Windows paths as this platform writes them (`path_grammar::native_text`, rule 1148).
        let native = |s: &str| {
            if cfg!(windows) {
                s.to_string()
            } else {
                s.replace("C:\\", "/C/").replace('\\', "/")
            }
        };
        let w = Wx::new(1.0);
        w.feed(&native(concat!(
            "\u{2502} 4  \u{2502} \u{27E6}C:\\Users\\me\\web-application-project\\app\\   \u{2502} \u{1F504}  \u{2502}\r\n",
            "\u{2502}    \u{2502} src\\app\\favorites\\favorites.ts\u{27E7}            \u{2502} now \u{2502}",
        )));
        light_paths(&w, 8..20);
        w.right_click(12, 1);
        w.pick("Copy path");
        w.pump_until("the path copy", |w| w.clip_text().is_some());
        assert_eq!(
            w.clip_text(),
            Some(native(
                "C:\\Users\\me\\web-application-project\\app\\src\\app\\favorites\\favorites.ts"
            ))
        );
        w.right_click(12, 0);
        w.pick("Open");
        w.pump_until("the word in the panel", |w| {
            w.h.comp.get_panel_title().as_str() == "No such file"
        });
        assert!(w.records().is_empty(), "{:?}", w.records());
    }

    /// The calling thread's own clock, the one the hold tests below bound: its CPU time where the system
    /// keeps it to the microsecond (macOS, Linux), so a shared runner that takes the core away in the
    /// middle of a tick does not count as the tick's work (an 88 ms tick of 18 ms work with every core
    /// busy twice over, dev Mac 2026-11-10); the wall clock on Windows, whose thread times move in
    /// 15.6 ms steps.
    #[cfg(unix)]
    fn thread_clock() -> Duration {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a live timespec the call fills in; the clock id is one every unix has.
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        assert_eq!(rc, 0, "the thread's CPU clock");
        Duration::new(
            u64::try_from(ts.tv_sec).unwrap_or(0),
            u32::try_from(ts.tv_nsec).unwrap_or(0),
        )
    }

    #[cfg(not(unix))]
    fn thread_clock() -> Duration {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed()
    }

    /// How long `f` took on [`thread_clock`].
    fn thread_time(f: impl FnOnce()) -> Duration {
        let t = thread_clock();
        f();
        thread_clock().saturating_sub(t)
    }

    /// How many times slower than the reference machine this one runs a fixed unit of plain
    /// unoptimised work (best of five on [`thread_clock`], never below 1). The unit is the same in every run.
    fn machine_slowness() -> f64 {
        /// Measured on the dev PC (debug build, idle priority): 8.2 ms.
        const REFERENCE_MS: f64 = 8.0;
        let best = (0..5)
            .map(|_| {
                thread_time(|| {
                    let mut acc = 0u64;
                    for i in 0..2_000_000u64 {
                        acc = std::hint::black_box(acc.wrapping_mul(31).wrapping_add(i));
                    }
                    std::hint::black_box(acc);
                })
                .as_secs_f64()
                    * 1000.0
            })
            .fold(f64::MAX, f64::min);
        eprintln!("calibration unit took {best:.3} ms");
        (best / REFERENCE_MS).max(1.0)
    }

    /// PT linkPaintStall: nothing holds the window while a big scrollback is scanned and searched.
    ///
    /// Bounded is what the scrollback and the search add to the UI thread's work ([`thread_clock`]): each
    /// tick (the find runs its slices there) and how much longer the grid host's frame takes than a plain
    /// repaint of the same window, measured in the same run. The plain frame (laying out 1000 x 604 pixels, painting them and
    /// handing them to Slint, unoptimised) is the size of the window, not of the scrollback: 27 ms on the dev
    /// Mac, of which 16 ms is the hand-over, and 80 to 130 ms on a shared macOS CI runner, which the
    /// calibration unit does not see (2026-11-10). A real hold, the whole search on the UI thread, is a
    /// 190 ms tick on the dev Mac where a sliced one is under 20 ms.
    #[test]
    fn find_and_a_big_scrollback_never_hold_the_window() {
        let w = Wx::new(1.0);
        let mut text = String::new();
        for i in 0..10_000 {
            text.push_str(&format!(
                "line {i} https://example.com/{i} C:\\Windows\\win.ini\r\n"
            ));
        }
        w.feed(&text);
        // A tick and the grid host's own frame are what the app does on its UI thread each 33 ms; the
        // window's software render of the whole 1000 x 640 window is the toolkit's and, unoptimised, not ours.
        for _ in 0..4 {
            w.step(50);
            w.paint_grid();
        }
        // What a frame of this window costs on this machine now: the middle of five repaints of the same grid.
        let mut plain: Vec<Duration> = (0..5)
            .map(|_| {
                w.host.borrow_mut().mark_dirty();
                thread_time(|| {
                    w.paint_grid();
                })
            })
            .collect();
        plain.sort();
        let plain = plain[2];
        let mut worst_tick = Duration::ZERO;
        let mut worst_extra = Duration::ZERO;
        let mut measure = |w: &Wx, ms: u64| {
            worst_tick = worst_tick.max(thread_time(|| w.step(ms)));
            let frame = thread_time(|| {
                w.paint_grid();
            });
            worst_extra = worst_extra.max(frame.saturating_sub(plain));
        };
        assert_eq!(w.key(ctrl("f", "KeyF")), KeyOutcome::Consumed);
        w.type_text("line");
        for _ in 0..600 {
            measure(&w, 50);
            if w.h.comp.get_find_readout().contains("1000+") {
                break;
            }
        }
        assert!(
            w.h.comp.get_find_readout().contains("1000+"),
            "the count reached {:?}",
            w.h.comp.get_find_readout()
        );
        // 50 ms on the machine this was tuned on; a slower machine (a shared CI runner, a loaded
        // PC) gets the same budget scaled by how much slower it runs a fixed unit of plain work. A
        // real hold (the scan running whole on the UI thread) is far beyond any such scaling.
        let factor = machine_slowness();
        let budget = Duration::from_millis(50).mul_f64(factor);
        eprintln!(
            "slowest tick {worst_tick:?}, a frame at most {worst_extra:?} over a plain one ({plain:?}), \
             budget {budget:?} (this machine is {factor:.2}x the reference)"
        );
        assert!(
            worst_tick < budget,
            "the slowest tick took {worst_tick:?} (budget {budget:?}, machine {factor:.2}x)"
        );
        assert!(
            worst_extra < budget,
            "a frame took {worst_extra:?} more than a plain one ({plain:?}; budget {budget:?}, machine {factor:.2}x)"
        );
    }

    // ---- find ---------------------------------------------------------------------------------

    /// PT scenario findKey.
    #[test]
    fn ctrl_f_opens_the_bar_escape_closes_it_and_a_full_screen_program_keeps_ctrl_f() {
        let w = Wx::new(1.0);
        w.feed("hello");
        let closed = w.frame();
        assert_eq!(w.key(ctrl("f", "KeyF")), KeyOutcome::Consumed);
        assert!(w.h.comp.get_find_open());
        let open = w.frame();
        let region = bar_region(&w, &open);
        let (x0, y0, x1, y1) = diff_box(&closed, &open, region).expect("the bar is drawn");
        assert!(x1 - x0 > w.h.phys(300.0), "the bar is about 334 px wide");
        // A hairline border, and no shadow outside it.
        for d in [1u32, 3, 6] {
            for p in [
                open.px(x0 - d, (y0 + y1) / 2),
                open.px((x0 + x1) / 2, y1 + d),
            ] {
                assert!(Frame::near(p, GROUND, 1), "shadow {d} px out: {p:?}");
            }
        }
        assert_eq!(w.escape(), KeyOutcome::Slint);
        assert!(!w.h.comp.get_find_open());
        let gone = w.frame();
        assert!(
            diff_box(&closed, &gone, region).is_none(),
            "the bar is gone"
        );
        // A full-screen program keeps Ctrl+F; Ctrl+Shift+F always finds.
        w.feed("\x1b[?1049h");
        assert_eq!(w.key(ctrl("f", "KeyF")), KeyOutcome::Pty);
        assert!(!w.h.comp.get_find_open());
        assert_eq!(
            w.key(KeyEvent::new("F", "KeyF").ctrl().shift()),
            KeyOutcome::Consumed
        );
        assert!(w.h.comp.get_find_open());
    }

    /// PT scenario findKey.
    #[test]
    fn typing_highlights_every_match_and_enter_steps() {
        let w = Wx::new(1.0);
        w.feed("err one\r\nerr two\r\nfine\r\nerr three");
        let closed = w.frame();
        w.key(ctrl("f", "KeyF"));
        w.type_text("err");
        w.frame_until("1 of 3", |w, _| w.h.comp.get_find_readout() == "1 of 3");
        let active_row = |w: &Wx| {
            let f = w.frame();
            let rows: Vec<usize> = [0usize, 1, 3]
                .into_iter()
                .filter(|&r| w.fill_in(&f, r, 0..3, ACTIVE_FILL) > 0)
                .collect();
            assert_eq!(rows.len(), 1, "exactly one active match: {rows:?}");
            for r in [0usize, 1, 3] {
                if r != rows[0] {
                    assert!(
                        w.fill_in(&f, r, 0..3, FIND_FILL) > 0,
                        "row {r} wears the match fill"
                    );
                    assert_eq!(w.fill_in(&f, r, 0..3, ACTIVE_FILL), 0);
                }
            }
            assert_eq!(w.fill_in(&f, 2, 0..4, FIND_FILL), 0, "no match on `fine`");
            rows[0]
        };
        let first = active_row(&w);
        w.key(KeyEvent::new("Enter", "Enter").text("\n"));
        w.frame_until("2 of 3", |w, _| w.h.comp.get_find_readout() == "2 of 3");
        let second = active_row(&w);
        assert_ne!(first, second, "Enter moves the active match");
        w.key(KeyEvent::new("Enter", "Enter").shift().text("\n"));
        w.frame_until("1 of 3", |w, _| w.h.comp.get_find_readout() == "1 of 3");
        assert_eq!(active_row(&w), first);
        // The readout is drawn right aligned in its 52 px box.
        let f = w.frame();
        let region = bar_region(&w, &f);
        let (x0, y0, _, y1) = diff_box(&closed, &f, region).expect("the bar is drawn");
        let hair = w.hair_phys();
        let box_x0 = x0 + hair + w.h.phys(8.0 + 180.0 + 6.0);
        let box_x1 = box_x0 + w.h.phys(52.0);
        let ink = |p: [u8; 4]| !Frame::near(p, [31, 31, 30, 255], 8);
        let b = f
            .bbox_where(box_x0, y0 + hair + 1, box_x1, y1 - hair, ink)
            .expect("the readout is drawn");
        assert!(
            (box_x1 - 1).abs_diff(b.2) <= 3,
            "right aligned: ink ends at {}, box at {}",
            b.2,
            box_x1 - 1
        );
        assert!(b.0 > box_x0 + 4, "and does not fill the box");
    }

    /// PT scenario reviewKeys.
    #[test]
    fn typing_stays_in_the_find_bar_while_output_streams() {
        let w = Wx::new(1.0);
        w.key(ctrl("f", "KeyF"));
        for (i, ch) in "abcdefgh".char_indices() {
            w.feed(&format!("line {i}\r\n"));
            w.step(50);
            w.type_text(&ch.to_string());
            assert_eq!(w.h.comp.get_find_query().as_str(), &"abcdefgh"[..=i]);
        }
        assert!(w.h.comp.get_find_open());
        assert_eq!(w.quiet(), b"", "the shell received none of the keys");
    }

    /// PT scenario reviewKeys.
    #[test]
    fn cd_lands_with_find_open() {
        let w = Wx::new(1.0);
        w.key(ctrl("f", "KeyF"));
        assert!(w.h.comp.get_find_open());
        w.print("\x1b]9;9;C:\\Windows\x07", |w| {
            w.ctl
                .borrow()
                .model()
                .active()
                .is_some_and(|t| t.cwd == "C:\\Windows")
        });
        w.run(300);
        assert!(
            w.h.comp.get_find_open(),
            "the bar stays open when the folder changes"
        );
    }

    // ---- paste --------------------------------------------------------------------------------

    const BLOCK: &[u8] = b"\x1b[200~a\rb\x1b[201~";

    /// PT paste: Ctrl+V, Ctrl+Shift+V and the menu's Paste each deliver the bracketed block once.
    #[test]
    fn ctrl_v_ctrl_shift_v_and_the_menu_paste_each_deliver_exactly_once() {
        let w = Wx::new(1.0);
        w.feed("\x1b[?2004h");
        w.clip.set_text("a\nb");
        assert_eq!(w.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        w.wait_written(BLOCK);
        assert_eq!(w.quiet(), BLOCK, "one key is one paste");
        assert_eq!(
            w.key(KeyEvent::new("V", "KeyV").ctrl().shift()),
            KeyOutcome::Consumed
        );
        w.wait_written(&[BLOCK, BLOCK].concat());
        w.right_click(3, 0);
        w.pick("Paste");
        w.wait_written(&[BLOCK, BLOCK, BLOCK].concat());
        assert_eq!(
            w.quiet().len(),
            3 * BLOCK.len(),
            "three routes, three blocks, no more"
        );
    }

    /// Nothing a paste holds can end the bracketed paste early or type a control.
    #[test]
    fn a_hostile_paste_is_inert() {
        let w = Wx::new(1.0);
        w.feed("\x1b[?2004h");
        w.clip.set_text("x\x1b[201~calc\r\x00y");
        assert_eq!(w.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        w.wait_written(b"\x1b[200~x[201~calc\ry\x1b[201~");
        assert_eq!(w.quiet(), b"\x1b[200~x[201~calc\ry\x1b[201~");
    }

    /// Copied files paste as quoted paths for the shell that RUNS in the tab.
    fn paste_files(shell: &'static str, files: &[&str]) -> Vec<u8> {
        let w = Wx::with(
            1.0,
            Opts {
                shell: Some(shell),
                ..Opts::default()
            },
        );
        assert_eq!(
            w.handle.shell_id().as_deref(),
            Some(shell),
            "the session ran this shell"
        );
        w.clip
            .set_files(files.iter().map(ToString::to_string).collect());
        assert_eq!(w.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        // The first write can take longer than `quiet`'s 80 ms on a loaded run (seen on macOS,
        // where the tab's font database is larger): wait up to 2 s for it, then for quiet.
        let t = Instant::now();
        while w.written().is_empty() && t.elapsed() < Duration::from_secs(2) {
            w.pump();
            std::thread::sleep(Duration::from_millis(3));
        }
        w.quiet()
    }

    #[test]
    fn copied_files_paste_quoted_paths_for_each_shell() {
        let plain = "C:\\My Dir\\a b.txt";
        assert_eq!(paste_files("pwsh", &[plain]), b"'C:\\My Dir\\a b.txt'");
        assert_eq!(paste_files("cmd", &[plain]), b"\"C:\\My Dir\\a b.txt\"");
        // A WSL tab gets the path as Linux sees it.
        assert_eq!(
            paste_files("wsl-Ubuntu", &[plain]),
            b"'/mnt/c/My Dir/a b.txt'"
        );
        // A name that would run something stays inert in every shell.
        let cmd_sub = "C:\\d\\$(calc).txt";
        assert_eq!(paste_files("pwsh", &[cmd_sub]), b"'C:\\d\\$(calc).txt'");
        assert_eq!(
            paste_files("wsl-Ubuntu", &[cmd_sub]),
            b"'/mnt/c/d/$(calc).txt'"
        );
        // cmd: `%` still acts inside double quotes, so nothing is pasted; `&` is safe inside them.
        assert_eq!(paste_files("cmd", &["C:\\d\\100%.txt"]), b"");
        assert_eq!(paste_files("cmd", &["C:\\a&b.txt"]), b"\"C:\\a&b.txt\"");
        // A quote in a PowerShell name is doubled, never closing the string.
        assert_eq!(paste_files("pwsh", &["C:\\it's.txt"]), b"'C:\\it''s.txt'");
    }

    /// PT scenario imagePaste.
    #[test]
    fn an_image_pastes_the_agent_key_and_shift_enter_is_a_newline_only_for_agents() {
        let w = Wx::new(1.0);
        w.clip.set_image(true);
        assert_eq!(w.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        w.wait_written(&[0x16]);
        // Text riding along with the image (an Excel copy) is for a plain prompt to take, and an agent
        // ignores it; from here the clipboard holds both.
        w.clip.set_text("ignored");
        // A Claude tab takes the image through its own key; the text riding along is ignored.
        w.agent.0.set(Some(AgentKind::Claude));
        assert_eq!(w.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        w.wait_written(b"\x16\x1bv");
        // Ctrl+Shift+V pastes the text, even with an image on the clipboard.
        assert_eq!(
            w.key(KeyEvent::new("V", "KeyV").ctrl().shift()),
            KeyOutcome::Consumed
        );
        w.wait_written(b"\x16\x1bvignored");
        // Shift+Enter: a newline for Claude, a backslash and Enter for Codex, plain Enter elsewhere.
        let enter = KeyEvent::new("Enter", "Enter").shift();
        assert_eq!(w.key(enter.clone()), KeyOutcome::Consumed);
        w.wait_written(b"\x16\x1bvignored\n");
        w.agent.0.set(Some(AgentKind::Codex));
        assert_eq!(w.key(enter.clone()), KeyOutcome::Consumed);
        w.wait_written(b"\x16\x1bvignored\n\\\r");
        w.agent.0.set(None);
        let n = w.written().len();
        assert_eq!(w.key(enter), KeyOutcome::Pty);
        w.wait_written(&[&b"\x16\x1bvignored\n\\\r"[..], b"\r"].concat());
        assert_eq!(w.quiet().len(), n + 1);
    }

    // ---- drop ---------------------------------------------------------------------------------

    /// PT scenario dropAndMenu.
    #[test]
    fn a_dropped_file_types_its_quoted_path_and_nothing_runs() {
        let w = Wx::new(1.0);
        let proj = w.dir.path().join("proj");
        std::fs::create_dir(&proj).unwrap(); // sync-ok: test code only, never the app
        w.ctl.borrow_mut().on_dropped_path(
            std::path::PathBuf::from("C:\\My Dir\\a b.txt"),
            (500.0, 300.0),
        );
        w.step(1000);
        w.wait_written(b"'C:\\My Dir\\a b.txt'");
        assert_eq!(
            w.quiet(),
            b"'C:\\My Dir\\a b.txt'",
            "the path is typed and Enter never is"
        );
        // Over the title row it is a tab that opens (M3), not typing.
        let rows = w.h.comp.get_tab_rows().row_count();
        w.ctl.borrow_mut().on_dropped_path(proj, (100.0, 10.0));
        w.pump_until("the new tab", |w| {
            w.h.comp.get_tab_rows().row_count() == rows + 1
        });
        // A name with an escape in it types nothing.
        let w = Wx::new(1.0);
        w.ctl
            .borrow_mut()
            .on_dropped_path(std::path::PathBuf::from("C:\\a\u{1b}b.txt"), (500.0, 300.0));
        w.step(1000);
        assert_eq!(w.quiet(), b"");
    }

    /// PT scenario dropAndMenu.
    #[test]
    fn a_drop_on_the_start_screen_types_nothing() {
        let w = Wx::new(1.0);
        let id = w.ctl.borrow().model().active().unwrap().id.clone();
        w.ctl.borrow_mut().request_close(&id);
        w.pump();
        if w.h.comp.get_dialog_open() {
            w.h.comp.invoke_dialog_confirm();
            w.pump();
        }
        assert_eq!(w.h.comp.get_tab_rows().row_count(), 0, "no tab is left");
        // The closed tab's shell is ended on a helper thread, which answers ConPTY's start-up queries
        // (ESC[1;1R ESC[?1;2c) into the dying pty: wait until it is killed and its pty has gone quiet,
        // so those bytes are not taken for typing by the drop below (a busy runner wrote them late).
        let t = Instant::now();
        let mut last = w.written();
        let mut calm = 0;
        while (!w.fake.killed() || calm < 10) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
            w.pump();
            let now = w.written();
            calm = if now == last { calm + 1 } else { 0 };
            last = now;
        }
        assert!(w.fake.killed(), "the closed tab's shell is ended");
        let before = w.written();
        w.ctl
            .borrow_mut()
            .on_dropped_path(std::path::PathBuf::from("C:\\a\\b.txt"), (500.0, 300.0));
        w.step(1000);
        assert_eq!(w.written(), before);
    }

    /// PT scenario dropAndMenu.
    #[test]
    fn the_menu_find_row_opens_the_bar() {
        let w = Wx::new(1.0);
        w.feed("hello");
        w.right_click(3, 1);
        w.pick("Find in scrollback");
        assert!(w.h.comp.get_find_open());
        assert!(w.h.comp.get_find_input_focused(), "the input has the keys");
        w.type_text("he");
        assert_eq!(w.h.comp.get_find_query().as_str(), "he");
        assert_eq!(w.quiet(), b"");
    }

    // ---- clipboard from the program -------------------------------------------------------------

    /// PT scenario termClipboard.
    #[test]
    fn osc_52_lands_with_utf8_intact_says_copied_and_a_read_gets_no_reply() {
        let w = Wx::new(1.0);
        w.print("\x1b]52;c;T1NDNTIgw6Y=\x07", |w| {
            w.clip_text().as_deref() == Some("OSC52 \u{e6}")
        });
        w.pump_until("the badge", |w| w.h.comp.get_copied_phase() == 1);
        assert!(
            w.handle.with_emu_quiet(|e| e.row_text(0)).trim().is_empty(),
            "nothing is drawn for the sequence"
        );
        // A read request is never answered.
        w.fake.feed(b"\x1b]52;c;?\x07");
        w.run(300);
        assert_eq!(w.written(), b"", "no reply to a clipboard read");
        // Only the clipboard selection `c` is written: `p` leaves it alone.
        w.fake.feed(b"\x1b]52;p;QUJD\x07");
        w.run(300);
        assert_eq!(w.clip_text().as_deref(), Some("OSC52 \u{e6}"));
    }

    /// PT appCopyNotice: a program that owns the mouse copies in its own way; the clipboard changing after
    /// a selecting release is announced, an unchanged one is not.
    #[test]
    fn a_program_selection_copy_is_announced() {
        let w = Wx::new(1.0);
        w.feed("hello world");
        w.feed(MOUSE_ON);
        // The copy watch hears that the program has the mouse on the controller's tick: one tick before
        // the press, or the press raced the window's real timer (it lost once on the Windows check job,
        // run 42612597989, and the press was not kept).
        w.run(50);
        w.raw_drag(0, 0, 5, false, false);
        w.clip.set_text("picked by the program");
        w.run(200);
        assert_eq!(w.h.comp.get_copied_phase(), 1, "the copy is announced");
        // The clipboard did not change: no badge, however long the looks go on.
        let w = Wx::new(1.0);
        w.feed("hello world");
        w.feed(MOUSE_ON);
        w.run(50);
        w.raw_drag(0, 0, 5, false, false);
        w.run(1800);
        assert_eq!(w.h.comp.get_copied_phase(), 0);
    }

    // ---- scaling ------------------------------------------------------------------------------

    /// PT scenario: (new, no Prism Terminal e2e counterpart).
    #[test]
    fn selection_lands_on_the_right_cells_at_every_scale() {
        for s in SCALES {
            let w = Wx::new(s);
            w.feed("\r\nabcdefghij");
            w.drag(1, 2, 7);
            assert_eq!(w.text(), "cdefg", "scale {s}");
            // And the highlight tiles exactly those five cells.
            let f = w.frame();
            let (x0, y0, _, ch) = w.cell_rect(2, 1);
            let (x7, _, _, _) = w.cell_rect(7, 1);
            assert_eq!(
                f.px(x0, y0),
                SELECTION,
                "scale {s}: the first cell's corner"
            );
            assert_eq!(
                f.px(x7 - 1, y0 + ch - 1),
                SELECTION,
                "scale {s}: the last cell's corner"
            );
            assert!(
                Frame::near(f.px(x0 - 1, y0), GROUND, 1),
                "scale {s}: left of the first cell"
            );
            assert!(
                Frame::near(f.px(x7, y0), GROUND, 1),
                "scale {s}: right of the last cell"
            );
        }
    }

    /// PT scenario: (new, no Prism Terminal e2e counterpart).
    #[test]
    fn a_pointer_over_a_link_changes_the_cursor_and_underlines_it() {
        let w = Wx::new(1.0);
        w.feed("see https://example.com/x now");
        w.settle();
        w.to(w.cell_at(1, 0, 0.5));
        w.pump();
        assert!(format!("{:?}", w.h.comp.get_grid_cursor()).contains("Text"));
        let (_, _, _, ch) = w.cell_rect(8, 0);
        let faint = w.frame();
        let idle = w.cell_px(&faint, 8, 0, 1, ch - 1);
        assert!(
            idle != LINK && !Frame::near(idle, GROUND, 2),
            "faint underline: {idle:?}"
        );
        w.to(w.cell_at(8, 0, 0.5));
        w.pump();
        assert!(format!("{:?}", w.h.comp.get_grid_cursor()).contains("Pointer"));
        let hot = w.frame();
        for col in 4..25 {
            assert_eq!(
                w.cell_px(&hot, col, 0, 1, ch - 1),
                LINK,
                "cell {col}: the underline is full ink"
            );
        }
        w.to(w.cell_at(1, 0, 0.5));
        w.pump();
        assert!(format!("{:?}", w.h.comp.get_grid_cursor()).contains("Text"));
    }

    impl Wx {
        /// The window moves to a monitor of another scaling: the same logical window (1000 x 604 css px of
        /// grid), a new scale factor, so a new device size. What the real window's `changed` handlers do:
        /// the grid host takes the size and the scale, and the controller the cell counts it gives.
        fn move_grid_to_scale(&self, scale: f32) {
            let (w, h) = (
                (1000.0 * scale).round() as u32,
                (604.0 * scale).round() as u32,
            );
            self.host.borrow_mut().resize(w, h, scale);
            let _ = self.host.borrow_mut().render();
            let (cols, rows) = self.host.borrow().dims();
            self.ctl.borrow_mut().on_grid_dims(cols, rows);
            self.pump();
        }

        /// A pixel of the grid host's own image (no window around it).
        fn grid_px(&self, x: u32, y: u32) -> [u8; 4] {
            self.host.borrow_mut().mark_dirty();
            self.host
                .borrow_mut()
                .render_rgba()
                .expect("a grid image")
                .pixel(x, y)
        }

        /// The five selected cells `c..g` of row 1 tile exactly: the corners are the selection blend, the
        /// pixels just outside are not.
        fn assert_tiles(&self, label: &str) {
            let m = self.metrics();
            let at = |col: u32, row: u32| (m.padding + col * m.cell_w, m.padding + row * m.cell_h);
            let (x0, y0) = at(2, 1);
            let (x7, _) = at(7, 1);
            assert_eq!(
                self.grid_px(x0, y0),
                SELECTION,
                "{label}: first cell corner"
            );
            assert_eq!(
                self.grid_px(x7 - 1, y0 + m.cell_h - 1),
                SELECTION,
                "{label}: last cell corner"
            );
            assert!(
                Frame::near(self.grid_px(x0 - 1, y0), GROUND, 1),
                "{label}: left of the highlight"
            );
            assert!(
                Frame::near(self.grid_px(x7, y0), GROUND, 1),
                "{label}: right of the highlight"
            );
        }
    }

    /// The window moves to a monitor of another scaling and back. The selection stands whatever the
    /// new column count is (owner decision O23, M5 plan D2, closed in M10 Task 19, register row
    /// DIFF-10: the anchors keep their rows and clamp their columns, as in xterm), and its highlight
    /// tiles the same cells at every scale. This test once expected another column count to clear
    /// the selection, which was alacritty's rule until Task 19 replaced it; the assertion follows
    /// the decision, it was not loosened. After the round trip a new selection works at the first
    /// scale.
    #[test]
    fn moving_between_monitors_keeps_the_cell_math() {
        let w = Wx::new(1.0);
        w.feed("\r\nabcdefghij");
        w.drag(1, 2, 7);
        assert_eq!(w.text(), "cdefg");
        w.assert_tiles("1.0");
        let cols = w.cols();
        let mut other_cols = 0;
        for s in &SCALES[1..] {
            w.move_grid_to_scale(*s);
            if w.cols() != cols {
                other_cols += 1;
            }
            assert!(w.cols() >= 7, "scale {s}: the selected cells still fit");
            assert_eq!(
                w.text(),
                "cdefg",
                "scale {s}: the selection stands, whatever the column count"
            );
            w.assert_tiles(&format!("scale {s}"));
            w.move_grid_to_scale(1.0);
            // Past the double-click time, so the next press is a first click.
            w.step(1000);
            w.drag(1, 2, 7);
            assert_eq!(
                w.text(),
                "cdefg",
                "scale {s}: a new selection works after the round trip"
            );
            w.assert_tiles(&format!("1.0 after {s}"));
        }
        assert!(
            other_cols >= 1,
            "at least one scale changes the column count, or this proves nothing about O23"
        );
    }

    /// Remote sessions PR 3: a remote tab with a fake link, in front. The host's output is drawn in the grid, and what
    /// is typed there reaches the link as keys and text, never the local shell (rule 279).
    #[test]
    fn a_remote_tab_shows_the_hosts_output_and_takes_typing() {
        let w = Wx::new(1.0);
        let before = w.written();
        let id = w
            .ctl
            .borrow_mut()
            .open_remote("mac", "/Users/max/proj", None, true);
        w.pump_until("the remote pty", |w| !w.remote.made().is_empty());
        let io = w.remote.made()[0].1.clone();
        let front = || w.slot.borrow().clone().expect("a tab in front");
        w.pump_until("the remote tab in front", |w| {
            w.ctl.borrow().model().active().map(|t| t.id.clone()) == Some(id.clone())
                && w.slot.borrow().as_ref().is_some_and(|h| h.id() == id)
        });
        assert!(w.ctl.borrow().model().remote_of(&id).is_some());
        io.push_output(b"hello from the host\r\n");
        w.pump_until("the host's text", |_| {
            front()
                .screen_tail(5)
                .join("\n")
                .contains("hello from the host")
        });
        let f = w.frame();
        let (x, y, cw, ch) = w.cell_rect(0, 0);
        assert!(
            f.count_where(x, y, x + 5 * cw, y + ch, |p| p != GROUND) > 0,
            "the host's text is drawn"
        );
        w.key_front(KeyEvent::new("l", "KeyL").text("l"));
        w.key_front(KeyEvent::new("s", "KeyS").text("s"));
        w.key_front(KeyEvent::new("Enter", "Enter"));
        let mut typed = Vec::new();
        let t = Instant::now();
        while typed.len() < 3 && t.elapsed() < Duration::from_secs(5) {
            typed.extend(io.take_outgoing(Duration::from_millis(20)).intents);
            w.pump();
        }
        use flare_term::handle::RemoteIntent;
        assert_eq!(
            typed,
            vec![
                RemoteIntent::Text("l".into()),
                RemoteIntent::Text("s".into()),
                RemoteIntent::Key(KeyEvent::new("Enter", "Enter")),
            ]
        );
        assert_eq!(w.quiet(), before, "nothing reached the local shell");
    }

    /// Remote sessions PR 5 (rule 299): the agent's `open.remote` leaves the tab in front where it is, so what the person
    /// types there never lands in the new remote session; the new tab is there behind it, its link started.
    #[test]
    fn an_agents_remote_open_stays_behind_the_tab_in_front() {
        let w = Wx::new(1.0);
        let was = w.ctl.borrow().model().active().map(|t| t.id.clone());
        assert!(was.is_some(), "a tab in front");
        let id = w
            .ctl
            .borrow_mut()
            .open_remote("mac", "/Users/max/proj", None, false);
        w.pump_until("the remote pty", |w| !w.remote.made().is_empty());
        assert_eq!(w.ctl.borrow().model().active().map(|t| t.id.clone()), was);
        assert!(w.ctl.borrow().model().remote_of(&id).is_some());
        w.key_front(KeyEvent::new("x", "KeyX").text("x"));
        w.pump();
        let io = w.remote.made()[0].1.clone();
        assert!(
            io.take_outgoing(Duration::from_millis(50))
                .intents
                .is_empty(),
            "nothing typed in front reached the remote session"
        );
    }
}
