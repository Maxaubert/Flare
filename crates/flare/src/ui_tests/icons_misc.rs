//! Icon tests for the start screen, the find bar, the title bar and the update chip (Task 52). Slint fits a stroked
//! Path into its box less one stroke, so each stroked glyph here is drawn in a box one stroke wider, moved back by
//! half a stroke (the Task 50 pattern, `icons_tabs.rs`). Every expected size comes from the glyph's own commands
//! (`expected_extent_px`), and every glyph is measured at all eight scalings. The scan at the end holds the list of
//! stroked paths in `ui/*.slint` against the glyphs these files test, so a new stroked path cannot skip its test.

use std::collections::BTreeMap;
use std::f32::consts::PI;

use slint::{ModelRc, VecModel};

use super::icons_tabs::{
    CHEVRON_DOWN, CLOSE, COG, ICON, Ink, expected_extent_px, ink_edges, region_around,
};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, FindBarPreview, Place, StartScreenPreview, UpdateChipPreview};

/// `Theme.ground` (the start screen, the find bar's window and the title bar), and the update chip's label ink.
const GROUND: [u8; 4] = [23, 23, 23, 255];
const ON_ACCENT: [f32; 3] = [11.0, 11.0, 15.0];

/// The glyph strings of `glyphs.slint` that these surfaces draw (the nav, the find bar, the title bar, the chip).
const PLUS_START: &str = "M12 5.5v13M5.5 12h13";
const FOLDER: &str =
    "M3.5 7.5a2 2 0 0 1 2-2h3.6l2 2.4h7.4a2 2 0 0 1 2 2v6.6a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z";
const CHEVRON_UP: &str = "M18 15l-6-6-6 6";
const MINIMIZE: &str = "M6 12h12";
const MAXIMIZE: &str = "M7.5 6h9a1.5 1.5 0 0 1 1.5 1.5v9a1.5 1.5 0 0 1-1.5 1.5h-9a1.5 1.5 0 0 1-1.5-1.5v-9a1.5 1.5 0 0 1 1.5-1.5z";
const TITLE_CLOSE: &str = "M7 7l10 10M17 7L7 17";
const HELP: &str = "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M9.4 9.3a2.7 2.7 0 1 1 4 2.4c-.9.5-1.4 1.1-1.4 2.1 M12 17.2v.01";
const UPDATE_ARROW: &str = "M12 4v11m0 0l-4.5-4.5M12 15l4.5-4.5M5 20h14";

/// A drawn glyph: its ink edges in device pixels and the size it should draw at.
type Drawn = ((f32, f32, f32, f32), (f32, f32));

/// The most common colour in a region: the ground behind a glyph (its strokes are thin, so they never win).
fn ground(f: &Frame, r: (u32, u32, u32, u32)) -> [u8; 4] {
    let mut n: BTreeMap<[u8; 4], u32> = BTreeMap::new();
    for y in r.1..r.3.min(f.h) {
        for x in r.0..r.2.min(f.w) {
            *n.entry(f.px(x, y)).or_default() += 1;
        }
    }
    n.into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(p, _)| p)
        .expect("the region holds pixels")
}

/// The ink edges of the glyph in `r` drawn in `ink` over the region's own ground.
fn drawn(f: &Frame, r: (u32, u32, u32, u32), ink: [f32; 3]) -> (f32, f32, f32, f32) {
    let bg = ground(f, r);
    ink_edges(f, r, &Ink::new(bg, ink)).expect("the glyph is drawn")
}

/// The drawn ink is the expected size within one device pixel.
fn assert_size(what: &str, s: f32, d: Drawn) {
    let (got, want) = d;
    let (w, h) = (got.2 - got.0, got.3 - got.1);
    assert!(
        (w - want.0).abs() <= 1.0,
        "{what} at {s}: drawn width {w} px, want {} px",
        want.0
    );
    assert!(
        (h - want.1).abs() <= 1.0,
        "{what} at {s}: drawn height {h} px, want {} px",
        want.1
    );
}

fn place(path: &str) -> Place {
    Place {
        path: path.into(),
        name: path.rsplit('\\').next().unwrap_or(path).into(),
        near: "C:\\work".into(),
        label: "".into(),
        pinned: false,
    }
}

// --- The start screen: the New row's plus and a place row's folder (StartRow, start.slint). ---

/// The nav is 340 css px wide, centred in a 600 px window: its left edge is at css x 130. A row's glyph sits 10 px in
/// and is centred on the 36 px row, so its centre is 18 px in from the left and down from the row's top.
const NAV_LEFT: f32 = 130.0;
const START_W: u32 = 600;

fn draw_start_glyph(s: f32, folder: bool) -> Drawn {
    let places = if folder {
        vec![place("C:\\a\\api")]
    } else {
        vec![]
    };
    let h = Harness::new(s, START_W, 600, || StartScreenPreview::new().unwrap());
    h.comp.set_places(ModelRc::new(VecModel::from(places)));
    h.comp.set_shown(true);
    h.advance(500);
    let new_y = h.comp.get_new_y();
    let top = if folder { new_y + 57.0 } else { new_y };
    let f = h.render();
    let r = region_around(&f, (NAV_LEFT + 18.0) * s, (top + 18.0) * s, 9.0 * s);
    let got = drawn(&f, r, ICON);
    let want = if folder {
        expected_extent_px(FOLDER, 16.0, 24.0, 1.6 * 16.0 / 24.0, s)
    } else {
        expected_extent_px(PLUS_START, 16.0, 24.0, 1.6 * 16.0 / 24.0, s)
    };
    (got, want)
}

#[test]
fn start_nav_glyphs_draw_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("start new row", s, draw_start_glyph(s, false));
        assert_size("start folder row", s, draw_start_glyph(s, true));
    }
}

// --- The find bar's three buttons (FindButton, find.slint): 20 px buttons, a 12 px glyph in each. ---

/// The bar's right edge, in css px, and its top, from the drawn box (the bar sits 12 px from the top right).
fn bar_right_top(f: &Frame, s: f32) -> (f32, f32) {
    let (_, y0, x1, _) = f
        .bbox_where(0, 0, f.w, f.h, |p| !Frame::near(p, GROUND, 2))
        .expect("the bar is drawn");
    ((x1 + 1) as f32 / s, y0 as f32 / s)
}

fn draw_find_glyph(s: f32, from_right: f32, glyph: &str) -> Drawn {
    let h = Harness::new(s, 800, 400, || FindBarPreview::new().unwrap());
    h.comp.set_open(true);
    h.comp.set_can_step(true);
    h.advance(0);
    let f = h.render();
    let (right, top) = bar_right_top(&f, s);
    // A button is 20 px with its glyph centred in it: 15 px down from the bar's top and `from_right` in from its edge.
    let r = region_around(&f, (right - from_right) * s, (top + 15.0) * s, 9.0 * s);
    let got = drawn(&f, r, ICON);
    (
        got,
        expected_extent_px(glyph, 12.0, 24.0, 2.2 * 12.0 / 24.0, s),
    )
}

#[test]
fn find_bar_glyphs_draw_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("find previous", s, draw_find_glyph(s, 71.0, CHEVRON_UP));
        assert_size("find next", s, draw_find_glyph(s, 45.0, CHEVRON_DOWN));
        assert_size("find close", s, draw_find_glyph(s, 19.0, CLOSE));
    }
}

// --- The title bar's buttons (TitleButton, app.slint): the 13 px glyphs at 1.8 stroke units, the cog and the
// Command help glyph at 15 px. ---

fn draw_title(s: f32, help: bool, cx: f32, glyph: &str, icon: f32, units: f32) -> Drawn {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.comp.set_help_visible(help);
    h.advance(0);
    let f = h.render();
    let r = region_around(&f, cx * s, 18.0 * s, 9.0 * s);
    let got = drawn(&f, r, ICON);
    (
        got,
        expected_extent_px(glyph, icon, 24.0, units * icon / 24.0, s),
    )
}

/// The minimise dash, measured by its ink area. A dash's ends are round caps less than a pixel thick, so their
/// faint tips never reach the coverage an edge scan reads (at 100 percent that scan is a pixel short of the dash).
/// Its area is its length times its thickness plus the two half discs of the caps, so the length is the area over the
/// thickness, less the caps' share, plus one thickness for the caps' extent.
fn draw_minimise(s: f32) -> Drawn {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    h.advance(0);
    let f = h.render();
    let r = region_around(&f, 906.0 * s, 18.0 * s, 9.0 * s);
    let ink = Ink::new(ground(&f, r), ICON);
    let thick = 1.8 * 13.0 / 24.0 * s;
    // The dash is symmetric about its centre, so the ink's centroid in x is the centre of the glyph.
    let (mut area, mut moment) = (0.0_f32, 0.0_f32);
    for y in r.1..r.3.min(f.h) {
        for x in r.0..r.2.min(f.w) {
            let c = ink.cov(f.px(x, y));
            area += c;
            moment += c * (x as f32 + 0.5);
        }
    }
    let extent = area / thick - PI * thick / 4.0 + thick;
    let centre = moment / area;
    let edges = drawn(&f, r, ICON);
    (
        (
            centre - extent / 2.0,
            edges.1,
            centre + extent / 2.0,
            edges.3,
        ),
        expected_extent_px(MINIMIZE, 13.0, 24.0, 1.8 * 13.0 / 24.0, s),
    )
}

#[test]
fn title_bar_glyphs_draw_at_full_size_at_every_scale() {
    // The buttons' centres, from the right: close 978, maximise 942, minimise 906, the cog 870, Command help 834
    // (only with the help button showing, which moves the rest nowhere).
    for s in SCALES {
        assert_size("title minimise", s, draw_minimise(s));
        assert_size(
            "title maximise",
            s,
            draw_title(s, false, 942.0, MAXIMIZE, 13.0, 1.8),
        );
        assert_size(
            "title close",
            s,
            draw_title(s, false, 978.0, TITLE_CLOSE, 13.0, 1.8),
        );
        assert_size(
            "title settings",
            s,
            draw_title(s, false, 870.0, COG, 15.0, 1.7),
        );
        assert_size(
            "title command help",
            s,
            draw_title(s, true, 834.0, HELP, 15.0, 1.8),
        );
    }
}

// --- The update chip's arrow (update.slint, the 11 px icon): already grown by one stroke, so this passes at once. ---

fn draw_chip_arrow(s: f32) -> Drawn {
    let h = Harness::new(s, 1000, 120, || {
        UpdateChipPreview::new().expect("the preview builds")
    });
    h.comp.set_shown(true);
    h.comp.set_label("Update 0.56.0".into());
    h.comp.set_aria("Update to 0.56.0".into());
    h.comp.set_tip("See what is new in 0.56.0".into());
    h.advance(0);
    let f = h.render();
    // The icon's box is 11 px at css (chip x + 10, chip centre): its centre is 5.5 px in from both.
    let cx = h.comp.get_chip_x() + h.comp.get_icon_x() + 5.5;
    let cy = h.comp.get_chip_y() + h.comp.get_icon_y() + 5.5;
    let r = region_around(&f, cx * s, cy * s, 8.0 * s);
    let got = drawn(&f, r, ON_ACCENT);
    (
        got,
        expected_extent_px(UPDATE_ARROW, 11.0, 24.0, 2.4 * 11.0 / 24.0, s),
    )
}

#[test]
fn update_chip_arrow_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("update arrow", s, draw_chip_arrow(s));
    }
}

// --- The panel's close button (PanelClose, panel.slint): a 20 px button with a 10 px glyph, 8 px in from the slot. ---

fn draw_panel_close(s: f32) -> Drawn {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    crate::appctl::AppCtl::push_panel_limits(&h.comp);
    h.comp.set_panel_open(true);
    h.comp.set_panel_note("x".into());
    h.advance(0);
    let f = h.render();
    let slot_right =
        h.comp.get_page_left() + h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w();
    let cx = slot_right - 8.0 - 10.0;
    let cy = h.comp.get_grid_top() + h.comp.get_panel_header_h() / 2.0;
    let r = region_around(&f, cx * s, cy * s, 8.0 * s);
    let got = drawn(&f, r, ICON);
    (
        got,
        expected_extent_px(CLOSE, 10.0, 24.0, 2.2 * 10.0 / 24.0, s),
    )
}

#[test]
fn panel_close_glyph_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("panel close", s, draw_panel_close(s));
    }
}

// --- The web panel's bar buttons (WebButton, panel.slint): a 24 px button with a 12 px glyph, 2 px apart, 8 px in from the slot. ---

const CHEVRON_LEFT: &str = "M15 6l-6 6 6 6";
const CHEVRON_RIGHT: &str = "M9 6l6 6-6 6";
const RELOAD: &str = "M19 12a7 7 0 1 1-2.05-4.95M19 5v4.5h-4.5";

fn draw_web_button(s: f32, which: u8, loading: bool) -> Drawn {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    crate::appctl::AppCtl::push_panel_limits(&h.comp);
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(3);
    h.comp.set_panel_web_can_back(true);
    h.comp.set_panel_web_can_forward(true);
    h.comp.set_panel_web_loading(loading);
    h.advance(0);
    let f = h.render();
    let left = h.comp.get_page_left() + h.comp.get_panel_slot_x();
    let cx = left + 8.0 + 12.0 + f32::from(which) * 26.0;
    let cy = h.comp.get_panel_slot_top() + h.comp.get_panel_header_h() + 16.0;
    let r = region_around(&f, cx * s, cy * s, 8.0 * s);
    let got = drawn(&f, r, ICON);
    let path = match (which, loading) {
        (0, _) => CHEVRON_LEFT,
        (1, _) => CHEVRON_RIGHT,
        (_, true) => CLOSE,
        _ => RELOAD,
    };
    (
        got,
        expected_extent_px(path, 12.0, 24.0, 2.2 * 12.0 / 24.0, s),
    )
}

#[test]
fn web_bar_glyphs_draw_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("web back", s, draw_web_button(s, 0, false));
        assert_size("web forward", s, draw_web_button(s, 1, false));
        assert_size("web reload", s, draw_web_button(s, 2, false));
        assert_size("web stop", s, draw_web_button(s, 2, true));
    }
}

// --- The conversation panels (convo.slint): the way back in a diff (10 px), a group's chevron (10 px) and the tick of
// a done task (8 px of a 16 box, inside a 14 px dot). ---

const TICK: &str = "M2 8.5l4 4 8-9";
/// `Theme.ground`: the tick is drawn in it, on the dot's grey.
const GROUND_INK: [f32; 3] = [23.0, 23.0, 23.0];

fn convo_window(s: f32) -> Harness<AppWindow> {
    let h = Harness::new(s, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    crate::appctl::AppCtl::push_panel_limits(&h.comp);
    h.comp.set_panel_open(true);
    h
}

fn draw_diff_back(s: f32) -> Drawn {
    let h = convo_window(s);
    h.comp.set_panel_kind(4);
    h.comp.set_panel_convo_selected("a.rs".into());
    h.advance(0);
    let f = h.render();
    // The bar is 30 px high; the glyph's box starts 6 px into the back button, which is 6 px in from the body.
    let cx = h.comp.get_panel_body_x() + 6.0 + 6.0 + 5.0;
    let cy = h.comp.get_panel_body_y() + 15.0;
    let r = region_around(&f, cx * s, cy * s, 8.0 * s);
    let got = drawn(&f, r, ICON);
    (
        got,
        expected_extent_px(CHEVRON_LEFT, 10.0, 24.0, 2.2 * 10.0 / 24.0, s),
    )
}

fn task_row(kind: i32, state: i32) -> crate::ConvoTaskRow {
    crate::ConvoTaskRow {
        kind,
        state,
        text: "x".into(),
        sub: "".into(),
        active: false,
        key: "k".into(),
        collapsed: false,
        count: "1 of 2 done".into(),
    }
}

fn draw_group_chevron(s: f32, folded: bool) -> Drawn {
    let h = convo_window(s);
    h.comp.set_panel_kind(5);
    let mut header = task_row(0, 0);
    header.collapsed = folded;
    h.comp
        .set_panel_convo_tasks(ModelRc::new(VecModel::from(vec![header])));
    h.advance(0);
    let f = h.render();
    let cx = h.comp.get_panel_body_x() + 10.0 + 5.0;
    let cy = h.comp.get_panel_body_y() + 15.0;
    let r = region_around(&f, cx * s, cy * s, 8.0 * s);
    let got = drawn(&f, r, ICON);
    let path = if folded { CHEVRON_RIGHT } else { CHEVRON_DOWN };
    (
        got,
        expected_extent_px(path, 10.0, 24.0, 2.2 * 10.0 / 24.0, s),
    )
}

fn draw_tick(s: f32) -> Drawn {
    let h = convo_window(s);
    h.comp.set_panel_kind(5);
    h.comp
        .set_panel_convo_tasks(ModelRc::new(VecModel::from(vec![task_row(1, 2)])));
    h.advance(0);
    let f = h.render();
    // The dot is 14 px at (16, 8) in its row; the tick is centred in it.
    let cx = h.comp.get_panel_body_x() + 16.0 + 7.0;
    let cy = h.comp.get_panel_body_y() + 8.0 + 7.0;
    let r = region_around(&f, cx * s, cy * s, 4.0 * s);
    let got = drawn(&f, r, GROUND_INK);
    (
        got,
        expected_extent_px(TICK, 8.0, 16.0, 2.4 * 8.0 / 16.0, s),
    )
}

#[test]
fn the_conversation_glyphs_draw_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("diff back", s, draw_diff_back(s));
        assert_size("group chevron open", s, draw_group_chevron(s, false));
        assert_size("group chevron folded", s, draw_group_chevron(s, true));
        assert_size("task tick", s, draw_tick(s));
    }
}

// --- The scan: every stroked path in the top-level ui files is either a hairline rule or a glyph with a size test. ---

/// Stroked paths per top-level `ui/*.slint` file that are glyphs with a size test (the test files named in each
/// file's comment). `tabs.slint`, `tabmark.slint`, `sidebar.slint` and `menus.slint` are covered by `icons_tabs.rs`
/// and `icons_side.rs`; the rest by this file.
const SIZED: [(&str, usize); 12] = [
    ("convo.slint", 3),
    ("tabs.slint", 5),
    ("tabmark.slint", 7),
    ("sidebar.slint", 3),
    ("menus.slint", 4),
    ("start.slint", 1),
    ("find.slint", 1),
    ("app.slint", 1),
    ("update.slint", 1),
    ("dictation_pill.slint", 1),
    ("dictation_widget.slint", 1),
    ("panel.slint", 2),
];

/// The hairline rules: straight rules or the sidebar's corner arcs, not glyphs, with no size test.
const HAIRLINES: [(&str, usize); 2] = [("app.slint", 1), ("sidebar.slint", 1)];

fn count_for(table: &[(&str, usize)], file: &str) -> usize {
    table
        .iter()
        .find(|(name, _)| *name == file)
        .map_or(0, |(_, n)| *n)
}

#[test]
fn every_stroked_path_has_a_size_test() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");
    let mut seen: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("the ui folder is read") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("slint") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("the file is read");
        let (mut glyphs, mut hairs) = (0usize, 0usize);
        for line in text.lines() {
            let t = line.trim_start();
            if !t.starts_with("stroke-width:") {
                continue;
            }
            if t.contains("Px.hair()") {
                hairs += 1;
            } else {
                glyphs += 1;
            }
        }
        seen.insert(name, (glyphs, hairs));
    }
    for (name, (glyphs, hairs)) in &seen {
        assert_eq!(
            *glyphs,
            count_for(&SIZED, name),
            "{name}: stroked glyph paths, each needs a size test listed in SIZED"
        );
        assert_eq!(
            *hairs,
            count_for(&HAIRLINES, name),
            "{name}: hairline rules, listed in HAIRLINES"
        );
    }
    for (name, n) in SIZED {
        let found = seen.get(name).map_or(0, |(g, _)| *g);
        assert_eq!(
            found, n,
            "{name} is listed in SIZED but has {found} stroked paths"
        );
    }
}

// --- The centres: a glyph whose box grew keeps the centre it had (recorded from the build before the change). ---

/// The centre of measured ink edges, in device pixels.
fn centre_of(got: (f32, f32, f32, f32)) -> (f32, f32) {
    ((got.0 + got.2) / 2.0, (got.1 + got.3) / 2.0)
}

/// Every glyph this file sizes, with the drawer that measures it.
type Measure = Box<dyn Fn(f32) -> Drawn>;

fn glyphs() -> Vec<(&'static str, Measure)> {
    vec![
        ("start new row", Box::new(|s| draw_start_glyph(s, false))),
        ("start folder row", Box::new(|s| draw_start_glyph(s, true))),
        (
            "find previous",
            Box::new(|s| draw_find_glyph(s, 71.0, CHEVRON_UP)),
        ),
        (
            "find next",
            Box::new(|s| draw_find_glyph(s, 45.0, CHEVRON_DOWN)),
        ),
        ("find close", Box::new(|s| draw_find_glyph(s, 19.0, CLOSE))),
        ("title minimise", Box::new(draw_minimise)),
        (
            "title maximise",
            Box::new(|s| draw_title(s, false, 942.0, MAXIMIZE, 13.0, 1.8)),
        ),
        (
            "title close",
            Box::new(|s| draw_title(s, false, 978.0, TITLE_CLOSE, 13.0, 1.8)),
        ),
        (
            "title settings",
            Box::new(|s| draw_title(s, false, 870.0, COG, 15.0, 1.7)),
        ),
        (
            "title command help",
            Box::new(|s| draw_title(s, true, 834.0, HELP, 15.0, 1.8)),
        ),
        ("update arrow", Box::new(draw_chip_arrow)),
    ]
}

/// The centres each glyph had before its box grew, in `SCALES` order, device pixels (recorded from the build before
/// the Task 52 change, measured by the same drawers as `centre_of_the_glyph_is_unchanged`).
const CENTRES_BEFORE: &[(&str, [(f32, f32); 8])] = &[
    (
        "start new row",
        [
            (148.00, 238.00),
            (185.00, 297.28),
            (222.00, 357.00),
            (258.99, 416.50),
            (296.00, 476.00),
            (333.00, 535.50),
            (370.00, 595.00),
            (444.00, 714.00),
        ],
    ),
    (
        "start folder row",
        [
            (148.00, 266.00),
            (185.00, 332.50),
            (222.00, 399.00),
            (259.00, 465.50),
            (296.00, 532.00),
            (333.00, 598.50),
            (370.00, 665.00),
            (444.00, 798.00),
        ],
    ),
    (
        "find previous",
        [
            (717.00, 27.04),
            (896.50, 33.42),
            (1076.00, 39.99),
            (1255.50, 46.49),
            (1433.99, 54.05),
            (1613.49, 60.49),
            (1793.00, 67.03),
            (2151.00, 81.03),
        ],
    ),
    (
        "find next",
        [
            (743.00, 26.95),
            (928.50, 33.58),
            (1115.00, 40.01),
            (1300.50, 46.51),
            (1485.99, 53.94),
            (1671.50, 60.50),
            (1858.00, 66.96),
            (2229.00, 80.97),
        ],
    ),
    (
        "find close",
        [
            (769.00, 27.00),
            (961.50, 33.50),
            (1154.00, 40.00),
            (1346.50, 46.50),
            (1538.00, 54.00),
            (1730.49, 60.49),
            (1923.00, 67.00),
            (2307.00, 81.00),
        ],
    ),
    (
        "title minimise",
        [
            (906.00, 18.00),
            (1132.50, 22.50),
            (1359.00, 27.00),
            (1585.50, 31.50),
            (1812.00, 36.00),
            (2038.50, 40.50),
            (2265.00, 45.00),
            (2718.00, 54.00),
        ],
    ),
    (
        "title maximise",
        [
            (942.00, 18.00),
            (1177.50, 22.50),
            (1413.00, 27.00),
            (1648.50, 31.50),
            (1884.00, 36.00),
            (2119.50, 40.50),
            (2355.00, 45.00),
            (2826.00, 54.00),
        ],
    ),
    (
        "title close",
        [
            (978.00, 18.00),
            (1222.50, 22.50),
            (1467.00, 27.00),
            (1711.50, 31.50),
            (1956.00, 36.00),
            (2200.50, 40.50),
            (2444.99, 44.99),
            (2933.99, 53.99),
        ],
    ),
    (
        "title settings",
        [
            (869.99, 18.01),
            (1087.49, 22.48),
            (1305.01, 26.97),
            (1522.49, 31.51),
            (1740.00, 35.99),
            (1957.50, 40.51),
            (2175.00, 44.99),
            (2609.99, 54.00),
        ],
    ),
    (
        "title command help",
        [
            (834.02, 18.02),
            (1042.51, 22.50),
            (1250.99, 27.00),
            (1459.50, 31.50),
            (1667.99, 36.00),
            (1876.50, 40.50),
            (2085.01, 45.01),
            (2501.99, 54.00),
        ],
    ),
    (
        "update arrow",
        [
            (741.50, 18.56),
            (926.87, 22.97),
            (1112.24, 27.33),
            (1297.68, 31.67),
            (1483.00, 36.11),
            (1668.38, 41.44),
            (1853.73, 44.81),
            (2224.50, 54.55),
        ],
    ),
];

#[test]
fn centre_of_the_glyph_is_unchanged() {
    for (what, draw) in glyphs() {
        let want = CENTRES_BEFORE
            .iter()
            .find(|(n, _)| *n == what)
            .unwrap_or_else(|| panic!("{what}: no recorded centre"))
            .1;
        for (s, w) in SCALES.into_iter().zip(want) {
            let c = centre_of(draw(s).0);
            assert!(
                (c.0 - w.0).abs() <= 0.5 && (c.1 - w.1).abs() <= 0.5,
                "{what} at {s}: centre {c:?}, before the change {w:?}"
            );
        }
    }
}
