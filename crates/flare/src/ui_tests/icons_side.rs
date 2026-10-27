//! Icon tests for the sidebar and the context menus (Task 51). Slint fits a stroked Path into its box less one
//! stroke, so each stroked glyph here must be drawn in a box one stroke wider than its nominal size, moved back
//! by half a stroke (the Task 50 pattern, `icons_tabs.rs`). The expected size comes from each glyph's own
//! commands. The collapsed sidebar's corner arc is a hairline rule, not a glyph, and has no size test.

use std::collections::HashMap;

use slint::{ModelRc, VecModel};

use super::icons_tabs::{
    CLOSE, ICON, Ink, TEXT, TEXT_SOFT, blend, expected_extent_px, ink_edges, region_around,
};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{MenuRow, MenusPreview, OverflowRow, TabRow, TabSidebarPreview};

/// `Theme.dim2` (the menu's quiet ink).
/// The sidebar X's stroke in css px: 2.4 units of its 10 px box (2.4 * 10 / 24 = 1, written out because clippy folds
/// the constants and then flags the equal operands).
const CLOSE_STROKE_PX: f32 = 1.0;

const DIM2: [f32; 3] = [113.0, 111.0, 106.0];

/// The glyph strings of `glyphs.slint` that the sidebar and the menus draw, and the menu's own test icon.
const SEARCH: &str = "M17.5 11a6.5 6.5 0 1 0-13 0a6.5 6.5 0 1 0 13 0zM16 16l4 4";
const PLUS_SIDE: &str = "M12 5v14m7-7H5";
const SEARCH_SMALL: &str = "M17 11a6 6 0 1 0-12 0a6 6 0 1 0 12 0zM20 20l-4.5-4.5";
const PIN: &str = "M15 3l6 6-3 1-4 4 .5 4.5L10 14l-6 6-1-1 6-6-4.5-4.5L9 8l4-4z";
const MENU_ICON: &str = "M4 6h16v12H4z";

/// A drawn glyph: its ink edges in device pixels and the size it should draw at.
type Drawn = ((f32, f32, f32, f32), (f32, f32));

fn tab(i: usize, name: &str, active: bool) -> TabRow {
    TabRow {
        id: format!("t{i}").into(),
        label: name.into(),
        tip: format!("C:\\work\\{name}").into(),
        folder: format!("~\\work\\{name}").into(),
        active,
        kept: true,
        prog: -1.0,
        ..TabRow::default()
    }
}

/// The sidebar as the Task 21 tests have it: six tabs, the second one active (its X is showing).
fn sidebar(s: f32) -> Harness<TabSidebarPreview> {
    let names = ["api", "web", "docs", "site", "tools", "notes"];
    let rows: Vec<TabRow> = names
        .iter()
        .enumerate()
        .map(|(i, n)| tab(i, n, i == 1))
        .collect();
    let h = Harness::new(s, 700, 500, || {
        TabSidebarPreview::new().expect("the preview builds")
    });
    h.comp.set_rows(ModelRc::new(VecModel::from(rows)));
    h.advance(0);
    h
}

fn row(label: &str) -> MenuRow {
    MenuRow {
        label: label.into(),
        hint: "".into(),
        icon: "".into(),
        filled_icon: false,
        disabled: false,
        trailing_icon: "".into(),
        trailing_filled: false,
        trailing_tip: "".into(),
        keep_open: false,
        group: false,
        flyout: false,
        flyout_pick: false,
        parent: -1,
    }
}

/// A context menu open at css (20, 20), with its panel position read back from the preview.
fn menu(s: f32, rows: Vec<MenuRow>) -> (Harness<MenusPreview>, f32, f32, f32) {
    let h = Harness::new(s, 400, 300, || MenusPreview::new().unwrap());
    h.comp.set_menu_rows(ModelRc::new(VecModel::from(rows)));
    h.comp.set_menu_x(20.0);
    h.comp.set_menu_y(20.0);
    h.comp.set_menu_open(true);
    h.advance(0);
    let (px, py, pw) = (
        h.comp.get_panel_x(),
        h.comp.get_panel_y(),
        h.comp.get_panel_w(),
    );
    (h, px, py, pw)
}

/// The most common colour in a region: the ground behind a glyph (strokes are thin, so they never win).
fn ground(f: &Frame, r: (u32, u32, u32, u32)) -> [u8; 4] {
    let mut n: HashMap<[u8; 4], u32> = HashMap::new();
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

/// The leftmost run of columns that hold ink in `r`, as `[start, end)`. A gap wider than `gap` device pixels
/// ends it, so a glyph is told apart from the text that follows it at the gap the layout leaves.
fn leftmost_cluster(f: &Frame, r: (u32, u32, u32, u32), ink: &Ink, gap: u32) -> Option<(u32, u32)> {
    let x1 = r.2.min(f.w);
    let has = |x: u32| (r.1..r.3.min(f.h)).any(|y| ink.cov(f.px(x, y)) > 0.2);
    let start = (r.0..x1).find(|&x| has(x))?;
    let mut last = start;
    for x in start..x1 {
        if has(x) {
            last = x;
        } else if x - last > gap {
            break;
        }
    }
    Some((start, last + 1))
}

/// The ink edges of the glyph at the left of `r`. The box is cut to the glyph's own columns and rows (two device
/// pixels of margin, so no faint tip is cut): a hairline rule near the glyph would otherwise read as ink.
fn draw_left_glyph(
    f: &Frame,
    r: (u32, u32, u32, u32),
    ink_colour: [f32; 3],
    gap: u32,
) -> (f32, f32, f32, f32) {
    let bg = ground(f, r);
    let ink = Ink::new(bg, ink_colour);
    let (x0, x1) = leftmost_cluster(f, r, &ink, gap).expect("the glyph is drawn");
    let rows: Vec<u32> = (r.1..r.3.min(f.h))
        .filter(|&y| (x0..x1).any(|x| ink.cov(f.px(x, y)) > 0.2))
        .collect();
    let (y0, y1) = (
        *rows.first().expect("the glyph has rows"),
        rows.last().expect("rows") + 1,
    );
    let padded = (
        x0.saturating_sub(2).max(r.0),
        y0.saturating_sub(2).max(r.1),
        (x1 + 2).min(r.2),
        (y1 + 2).min(r.3),
    );
    ink_edges(f, padded, &ink).expect("the glyph has ink edges")
}

fn draw_sidebar_close(s: f32) -> Drawn {
    // Row 1 is active, so its X shows: its 18 px box is css x 186..204, y 114..132.
    let h = sidebar(s);
    let f = h.render();
    let r = region_around(&f, 195.0 * s, 123.0 * s, 9.0 * s);
    let bg = ground(&f, r);
    let got = ink_edges(&f, r, &Ink::new(bg, ICON)).expect("the X is drawn");
    (
        got,
        expected_extent_px(CLOSE, 10.0, 24.0, CLOSE_STROKE_PX, s),
    )
}

fn draw_sidebar_search(s: f32) -> Drawn {
    // The find box: 8 px margins, 10 px padding, the 14 px glyph at css x 18, y 17..31.
    let h = sidebar(s);
    let f = h.render();
    let r = (
        (12.0 * s) as u32,
        (12.0 * s) as u32,
        (40.0 * s) as u32,
        (36.0 * s) as u32,
    );
    let got = draw_left_glyph(&f, r, ICON, (4.0 * s) as u32);
    (
        got,
        expected_extent_px(SEARCH, 14.0, 24.0, 1.8 * 14.0 / 24.0, s),
    )
}

fn draw_sidebar_plus(s: f32) -> Drawn {
    // The New tab button: the 13 px plus sits left of its text, 8 px away.
    let h = sidebar(s);
    let f = h.render();
    let r = (
        (8.0 * s) as u32,
        (452.0 * s) as u32,
        (212.0 * s) as u32,
        (480.0 * s) as u32,
    );
    let got = draw_left_glyph(&f, r, TEXT, (4.0 * s) as u32);
    (
        got,
        expected_extent_px(PLUS_SIDE, 13.0, 24.0, 2.2 * 13.0 / 24.0, s),
    )
}

const CHEVRON_DOWN: &str = "M6 9l6 6 6-6";

fn draw_sidebar_arrow(s: f32) -> Drawn {
    // The New tab button's arrow: the last 40 px of the button (css x 172..212), the 11 px chevron in its middle.
    let h = sidebar(s);
    let f = h.render();
    let r = region_around(&f, 192.0 * s, 475.0 * s, 9.0 * s);
    let bg = ground(&f, r);
    let got = ink_edges(&f, r, &Ink::new(bg, TEXT_SOFT)).expect("the chevron is drawn");
    (
        got,
        expected_extent_px(CHEVRON_DOWN, 11.0, 24.0, 2.2 * 11.0 / 24.0, s),
    )
}

fn draw_menu_icon(s: f32) -> Drawn {
    let mut r0 = row("One");
    r0.icon = MENU_ICON.into();
    let (h, px, py, _) = menu(s, vec![r0, row("Two")]);
    let f = h.render();
    // The icon is the first thing in row 0: 11 px of padding inside the panel's 1 px border.
    let r = (
        ((px + 1.0) * s) as u32,
        ((py + 2.0) * s) as u32,
        ((px + 41.0) * s) as u32,
        ((py + 26.0) * s) as u32,
    );
    let bg = ground(&f, r);
    let ink = blend(bg, TEXT_SOFT, 0.8);
    let got = draw_left_glyph(&f, r, ink, (4.0 * s) as u32);
    (
        got,
        expected_extent_px(MENU_ICON, 13.0, 24.0, 1.7 * 13.0 / 24.0, s),
    )
}

fn draw_menu_trailing(s: f32) -> Drawn {
    let mut r0 = row("One");
    r0.trailing_icon = PIN.into();
    let (h, px, py, pw) = menu(s, vec![r0, row("Two")]);
    let f = h.render();
    // The 20 px button's glyph: the test's centre for the trailing pin (18 css px in from the panel's right).
    let r = region_around(&f, (px + pw - 18.0) * s, (py + 15.0) * s, 9.0 * s);
    let bg = ground(&f, r);
    let got = ink_edges(&f, r, &Ink::new(bg, DIM2)).expect("the pin is drawn");
    (
        got,
        expected_extent_px(PIN, 12.0, 24.0, 1.8 * 12.0 / 24.0, s),
    )
}

const CHEVRON: &str = "M9 6l6 6-6 6";

fn draw_menu_chevron(s: f32) -> Drawn {
    let mut r0 = row("One");
    r0.flyout = true;
    let mut child = row("Child");
    child.parent = 0;
    let (h, px, py, pw) = menu(s, vec![r0, row("Two"), child]);
    let f = h.render();
    // The 11 px chevron ends 11 px of padding in from the panel's inner right edge.
    let hair = s.floor().max(1.0) / s;
    let r = region_around(
        &f,
        (px + pw - hair - 11.0 - 5.5) * s,
        (py + hair + 14.0) * s,
        9.0 * s,
    );
    let bg = ground(&f, r);
    let got =
        ink_edges(&f, r, &Ink::new(bg, blend(bg, TEXT_SOFT, 0.7))).expect("the chevron is drawn");
    (
        got,
        expected_extent_px(CHEVRON, 11.0, 24.0, 2.2 * 11.0 / 24.0, s),
    )
}

fn draw_search_small(s: f32) -> Drawn {
    let orow = OverflowRow {
        id: "t0".into(),
        label: "tab t0".into(),
        folder: "C:\\work".into(),
        needs: true,
        loading: false,
        settings: false,
        mark_state: 0,
        mark_color: slint::Color::from_argb_u8(0, 0, 0, 0),
        agent: 0,
    };
    let h = Harness::new(s, 400, 300, || MenusPreview::new().unwrap());
    h.comp.set_ov_rows(ModelRc::new(VecModel::from(vec![orow])));
    h.comp.set_ov_right(450.0);
    h.comp.set_ov_bottom(36.0);
    h.comp.set_ov_open(true);
    h.advance(0);
    let (bx, by) = (h.comp.get_ov_box_x(), h.comp.get_ov_box_y());
    let f = h.render();
    // The list's search row: 12 px padding in, 8 px down; the 13 px glyph sits inside the header.
    let r = (
        ((bx + 8.0) * s) as u32,
        ((by + 4.0) * s) as u32,
        ((bx + 40.0) * s) as u32,
        ((by + 24.0) * s) as u32,
    );
    let got = draw_left_glyph(&f, r, DIM2, (4.0 * s) as u32);
    (
        got,
        expected_extent_px(SEARCH_SMALL, 13.0, 24.0, 2.0 * 13.0 / 24.0, s),
    )
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

fn centre_of(got: (f32, f32, f32, f32)) -> (f32, f32) {
    ((got.0 + got.2) / 2.0, (got.1 + got.3) / 2.0)
}

#[test]
fn sidebar_close_x_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("sidebar close X", s, draw_sidebar_close(s));
    }
}

#[test]
fn sidebar_search_glyph_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("sidebar search", s, draw_sidebar_search(s));
    }
}

#[test]
fn sidebar_new_tab_arrow_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("sidebar new tab arrow", s, draw_sidebar_arrow(s));
    }
}

#[test]
fn sidebar_plus_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("sidebar plus", s, draw_sidebar_plus(s));
    }
}

#[test]
fn menu_icon_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("menu icon", s, draw_menu_icon(s));
    }
}

#[test]
fn menu_trailing_pin_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("menu trailing pin", s, draw_menu_trailing(s));
    }
}

#[test]
fn menu_flyout_chevron_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("menu flyout chevron", s, draw_menu_chevron(s));
    }
}

#[test]
fn overflow_search_glyph_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        assert_size("overflow search", s, draw_search_small(s));
    }
}

/// The six glyphs keep the centre they had before their boxes grew: the centres are device pixels recorded from
/// the build before the change, in `SCALES` order, each within half a pixel.
#[test]
fn centre_of_the_glyph_is_unchanged() {
    type Measure = fn(f32) -> Drawn;
    type Centres = [(f32, f32); 8];
    let before: [(&str, Measure, Centres); 6] = [
        ("sidebar close X", draw_sidebar_close, CENTRES_CLOSE),
        ("sidebar search", draw_sidebar_search, CENTRES_SEARCH),
        ("sidebar plus", draw_sidebar_plus, CENTRES_PLUS),
        ("menu icon", draw_menu_icon, CENTRES_MENU_ICON),
        ("menu trailing pin", draw_menu_trailing, CENTRES_PIN),
        ("overflow search", draw_search_small, CENTRES_SEARCH_SMALL),
    ];
    for (what, draw, centres) in before {
        for (s, want) in SCALES.into_iter().zip(centres) {
            let c = centre_of(draw(s).0);
            assert!(
                (c.0 - want.0).abs() <= 0.5 && (c.1 - want.1).abs() <= 0.5,
                "{what} at {s}: centre {c:?}, before the change {want:?}"
            );
        }
    }
}

const CENTRES_CLOSE: [(f32, f32); 8] = [
    (195.0, 123.0),
    (243.75, 153.75),
    (292.5, 184.5),
    (341.24, 215.24),
    (390.0, 246.0),
    (438.75, 276.75),
    (487.5, 307.5),
    (585.0, 369.0),
];
const CENTRES_SEARCH: [(f32, f32); 8] = [
    (25.02, 26.03),
    (31.36, 32.62),
    (37.68, 39.14),
    (43.92, 45.69),
    (50.22, 52.23),
    (56.48, 58.73),
    (62.78, 65.28),
    (75.32, 78.32),
];
/// The sidebar plus sits 20 css px further left than it was recorded (2026-10-27): its label is centred in the
/// button's main part, the button less its 40 px arrow.
const CENTRES_PLUS: [(f32, f32); 8] = [
    (62.0, 475.0),
    (77.5, 593.75),
    (93.0, 712.5),
    (108.5, 831.25),
    (124.0, 950.0),
    (139.5, 1068.75),
    (155.0, 1187.49),
    (186.0, 1425.0),
];
const CENTRES_MENU_ICON: [(f32, f32); 8] = [
    (38.5, 35.0),
    (47.85, 43.5),
    (57.26, 52.0),
    (66.62, 60.5),
    (77.0, 69.99),
    (86.37, 78.5),
    (95.75, 87.0),
    (115.5, 105.0),
];
const CENTRES_PIN: [(f32, f32); 8] = [
    (160.03, 34.76),
    (199.66, 43.17),
    (239.48, 51.59),
    (279.23, 60.06),
    (319.98, 69.53),
    (359.76, 77.99),
    (399.55, 86.43),
    (480.0, 104.28),
];
const CENTRES_SEARCH_SMALL: [(f32, f32); 8] = [
    (70.61, 57.22),
    (88.3, 71.29),
    (106.05, 85.59),
    (123.76, 99.75),
    (141.42, 114.42),
    (159.11, 128.64),
    (176.82, 142.81),
    (212.16, 171.71),
];
