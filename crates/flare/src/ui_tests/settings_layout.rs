//! UI tests of the Settings layout at eight scales (M7 Task 45); headless only.
//!
//! Every test builds the REAL page model (a fake `PageEnv` over memory stores), lays it out with
//! `flare_settings::layout`, fills `SettingsBridge` the way `SettingsCtl::push` does (through the same
//! `to_slint_section` mapper) and renders the `SettingsPreview` with the headless software platform.
//! The numbers are held to `pt_settings_metrics.json` (Prism Terminal measured in Task 3) within 1 css px.
//! `Scene` is shared with `settings_look.rs`.

use std::collections::HashMap;

use flare_settings::colour_format::colour_format;
use flare_settings::icons::icon_path;
use flare_settings::kv::PrefStore;
use flare_settings::layout::{
    self, BlockKind as LayoutBlock, PageLayout, RailLayout, SectionLayout, about_y, layout_page,
    rail_layout, rail_width,
};
use flare_settings::look::LookStore;
use flare_settings::page::{
    DiagInfo, GlassSupport, PageEnv, PageModel, PlaceholderDictation, ThemeColours, build_page,
};
use flare_settings::pages::PAGES;
use flare_settings::verb::VerbState;
use flare_settings::wall::{self, WallLayout};
use flare_theme::colour::{Rgba, parse_colour};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::settings_ctl::{RowCtx, to_slint_section};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{
    ColourPreview, ControlsPreview, RailItem, SelectPreview, SettingsBridge, SettingsPreview,
    SliderPreview, WallCard,
};

/// The pages whose geometry Prism Terminal's file holds.
pub(super) const MEASURED: [&str; 5] = ["appearance", "terminal", "agents", "diagnostics", "about"];
/// The frame widths the tests use (css px, rail included): narrow, 900, the reference 1000, 1600.
pub(super) const WIDTHS: [u32; 4] = [759, 900, 1000, 1600];
/// The wall's side paddings inside the panel (as `settings_ctl::WALL_SIDE_PADS`).
const WALL_SIDE_PADS: f64 = 32.0;
const BADGE_H: f64 = 112.0;

pub(super) fn colour_of(s: &str) -> slint::Color {
    let b = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    match parse_colour(s) {
        Some(Rgba { r, g, b: bl, a }) => {
            slint::Color::from_argb_u8(b(a * 255.0), b(r), b(g), b(bl))
        }
        None => slint::Color::from_argb_u8(0, 0, 0, 0),
    }
}

pub(super) fn page_id(page: &str) -> &'static str {
    PAGES
        .iter()
        .find(|p| p.id == page)
        .map(|p| p.id)
        .expect("a known page")
}

/// A page, built, laid out and drawn in a `SettingsPreview` at one scale and one frame size.
pub(super) struct Scene {
    pub h: Harness<SettingsPreview>,
    pub model: PageModel,
    pub layout: PageLayout,
    pub wall: WallLayout,
    pub rail: RailLayout,
    pub scale: f64,
    pub frame_w: f64,
    pub frame_h: f64,
}

pub(super) fn build(page: &str, prefs: &PrefStore, look: &LookStore) -> PageModel {
    let face = |_: &str| String::new();
    let env = PageEnv {
        prefs,
        look,
        shells: Some(vec![
            ("pwsh".to_string(), "PowerShell".to_string()),
            ("cmd".to_string(), "Command Prompt".to_string()),
        ]),
        verb: VerbState::Off,
        home: "C:/Users/Test".to_string(),
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        glass: GlassSupport::Ok,
        diag: DiagInfo {
            verbose: false,
            dir: "C:/logs".to_string(),
        },
        diag_marked: false,
        nvidia: false,
        dictation_wired: false,
        theme_colours: ThemeColours {
            accent: "#fe8f34".into(),
            background: "#101010".into(),
            secondary: "#181818".into(),
        },
        agent_colours: ["#111111".into(), "#222222".into(), "#333333".into()],
        agent_key_labels: String::new(),
        face_of: &face,
    };
    build_page(page, &env, &PlaceholderDictation)
}

/// The wall's layout in a frame `frame_w` wide, the way `SettingsCtl::wall_for` computes it.
pub(super) fn wall_at(frame_w: f64, expanded: bool, custom: bool) -> (WallLayout, f64) {
    let m = layout::Metrics::prism_terminal();
    let narrow = layout::is_narrow(frame_w);
    let pad_x = if narrow { m.pad_x_narrow } else { m.pad_x };
    let pane = frame_w - rail_width(frame_w);
    let content = (m.column_max.min(pane) - 2.0 * pad_x).max(0.0);
    let width = (content - 2.0 * m.border - WALL_SIDE_PADS).max(0.0);
    let total = wall::total_themes(custom);
    let base = wall::layout(width, total, expanded);
    let full = wall::layout(width, total, true);
    (base, full.full_h)
}

impl Scene {
    pub fn new(page: &str, scale: f32, frame_w: u32, frame_h: u32) -> Scene {
        Scene::with_prefs(page, scale, frame_w, frame_h, PrefStore::memory())
    }

    pub fn with_prefs(
        page: &str,
        scale: f32,
        frame_w: u32,
        frame_h: u32,
        prefs: PrefStore,
    ) -> Scene {
        let page = page_id(page);
        let look = LookStore::new(prefs.clone());
        let model = build(page, &prefs, &look);
        let (fw, fh, sc) = (f64::from(frame_w), f64::from(frame_h), f64::from(scale));
        let (wall, wall_full_h) = wall_at(fw, false, look.custom().is_some());
        let lay = layout_page(&model, fw, sc, &wall, wall_full_h, BADGE_H);
        let rail = rail_layout(fw, sc, PAGES.len());
        let h = Harness::new(scale, frame_w, frame_h, || {
            SettingsPreview::new().expect("the preview builds")
        });
        h.advance(0);
        let hair = 1.0_f64.max(sc.floor()) / sc;
        let ctx = RowCtx {
            format: colour_format(&prefs),
            drafts: &HashMap::new(),
            hair,
            flash_row: None,
            flash_section: None,
            flash: 0.0,
        };
        let b = h.comp.global::<SettingsBridge>();
        let end_y = about_y(&rail, fh);
        let items: Vec<RailItem> = PAGES
            .iter()
            .enumerate()
            .map(|(i, p)| RailItem {
                id: p.id.into(),
                label: p.label.into(),
                icon: icon_path(p.icon).unwrap_or("").into(),
                end: p.end,
                chosen: p.id == page,
                y: (if p.end { end_y } else { rail.items[i].0 }) as f32,
            })
            .collect();
        b.set_rail(ModelRc::new(VecModel::from(items)));
        b.set_narrow(rail.narrow);
        b.set_page_title(
            PAGES
                .iter()
                .find(|p| p.id == page)
                .map_or("", |p| p.label)
                .into(),
        );
        b.set_content_h(lay.content_h as f32);
        let sections: Vec<crate::SetSection> = model
            .sections
            .iter()
            .zip(&lay.sections)
            .map(|(s, l)| to_slint_section(s, l, &ctx))
            .collect();
        b.set_sections(ModelRc::new(VecModel::from(sections)));
        let total = wall::total_themes(look.custom().is_some());
        let (aria, title) = wall::toggle_texts(false, total);
        b.set_wall_expanded(false);
        b.set_wall_toggle_aria(aria.into());
        b.set_wall_toggle_title(title.into());
        b.set_wall_clip_h(wall.clip_h as f32);
        b.set_wall_full_h(wall_full_h as f32);
        let cards: Vec<WallCard> = wall::wall_cards(&look)
            .iter()
            .zip(&wall.positions)
            .map(|(c, (x, y))| WallCard {
                id: c.id.as_str().into(),
                name: c.name.as_str().into(),
                on: c.on,
                edit: c.edit,
                edit_name: "".into(),
                bg: colour_of(&c.bg),
                fg: colour_of(&c.fg),
                cursor: colour_of(&c.cursor),
                green: colour_of(&c.green),
                yellow: colour_of(&c.yellow),
                blue: colour_of(&c.blue),
                cyan: colour_of(&c.cyan),
                red: colour_of(&c.red),
                x: *x as f32,
                y: *y as f32,
            })
            .collect();
        b.set_wall_cards(ModelRc::new(VecModel::from(cards)));
        h.advance(0);
        Scene {
            h,
            model,
            layout: lay,
            wall,
            rail,
            scale: sc,
            frame_w: fw,
            frame_h: fh,
        }
    }

    /// A css length as a device pixel index.
    pub fn dev(&self, css: f64) -> u32 {
        (css * self.scale).round().max(0.0) as u32
    }

    /// Device pixels of one hairline: `max(1, floor(scale))` (the Chromium rule `Px.hair()` follows).
    pub fn hair_dev(&self) -> u32 {
        self.scale.floor().max(1.0) as u32
    }

    /// One hairline in css px.
    pub fn hair(&self) -> f64 {
        f64::from(self.hair_dev()) / self.scale
    }

    pub fn rail_w(&self) -> f64 {
        rail_width(self.frame_w)
    }

    /// The left edge (frame coordinates) of a panel's rows: rail, page padding, the panel's border.
    pub fn row_left(&self, s: &SectionLayout) -> f64 {
        self.rail_w() + s.panel_x + self.hair()
    }

    pub fn render(&self) -> Frame {
        self.h.render()
    }

    /// The panel's fill: a pixel of the first row's left padding, below its top edge.
    pub fn panel_fill(&self, f: &Frame, s: &SectionLayout) -> [u8; 4] {
        let r = &s.rows[0];
        f.px(
            self.dev(self.row_left(s)) + self.dev(4.0),
            self.dev(r.y) + self.dev(3.0),
        )
    }
}

/// Every Row-block (section index, row index) of the layout, in order.
pub(super) fn row_blocks(l: &PageLayout) -> Vec<(usize, usize)> {
    l.sections
        .iter()
        .enumerate()
        .flat_map(|(si, s)| {
            s.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.block == LayoutBlock::Row)
                .map(move |(ri, _)| (si, ri))
        })
        .collect()
}

fn whole(v: f64, scale: f64) -> bool {
    let d = v * scale;
    (d - d.round()).abs() < 1e-6
}

// ---- Prism Terminal's measured file ----------------------------------------------------------------

const METRICS: &str = include_str!("../../tests/fixtures/pt_settings_metrics.json");

fn fixture() -> serde_json::Value {
    serde_json::from_str(METRICS).expect("the metrics fixture is JSON")
}

/// `(x, y, width, height)` of one measured element.
fn pt_box(scale: &str, page: &str, name: &str) -> (f64, f64, f64, f64) {
    let v = fixture();
    let e = &v["scales"][scale][page][name];
    let n = |k: &str| {
        e[k].as_f64()
            .unwrap_or_else(|| panic!("{scale}/{page}/{name}/{k} is a number"))
    };
    (n("x"), n("y"), n("width"), n("height"))
}

/// The frame width Prism Terminal was measured in at one scale: 1000 css px at 100 percent, and the
/// whole device pixels of a 1500 px window at 150 percent (1003.33), which the layout takes rounded.
fn fx_w(key: &str) -> u32 {
    pt_box(key, "appearance", "frame").2.round() as u32
}

/// The value (css px) of one `SettingsShape` property in `theme.slint`.
fn shape(name: &str) -> f64 {
    let src = include_str!("../../ui/theme.slint");
    let start = src.find("export global SettingsShape").expect("the global");
    let body = &src[start..];
    let line = body
        .lines()
        .find(|l| l.contains(&format!("property <length> {name}:")))
        .unwrap_or_else(|| panic!("SettingsShape.{name} exists"));
    let value = line
        .split(':')
        .nth(1)
        .and_then(|v| v.split("px").next())
        .expect("a value");
    value.trim().parse().expect("a number")
}

fn within(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ---- tests -----------------------------------------------------------------------------------------

#[test]
fn rows_are_at_least_58_and_tiles_32_and_hairlines_start_60_in() {
    for s in SCALES {
        for page in ["appearance", "terminal"] {
            let sc = Scene::new(page, s, 1000, 604);
            let one_dev = 1.0 / sc.scale;
            // The model: every row block is 58 css px, to the device pixel.
            for (si, ri) in row_blocks(&sc.layout) {
                let r = &sc.layout.sections[si].rows[ri];
                assert!(
                    within(r.h, 58.0, one_dev),
                    "scale {s} {page}: row {} is {} high, not 58",
                    r.id,
                    r.h
                );
                assert!(
                    r.h * sc.scale >= (58.0 * sc.scale).floor() - 1e-6,
                    "scale {s} {page}: row {} is at least 58",
                    r.id
                );
            }
            // The frame: the tile is 32 square, 14 in, and the hairline starts 60 in.
            let f = sc.render();
            let Some((si, sec)) = sc
                .layout
                .sections
                .iter()
                .enumerate()
                .find(|(_, x)| x.rows.len() >= 2 && x.rows[1].block == LayoutBlock::Row)
            else {
                panic!("scale {s} {page}: a section with two rows");
            };
            let _ = si;
            let left = sc.dev(sc.row_left(sec));
            let fill = sc.panel_fill(&f, sec);
            // The second row's hairline: its first device column that is not the panel's fill.
            let y = sc.dev(sec.rows[1].y);
            let mut first = None;
            for x in (left + sc.dev(4.0))..(left + sc.dev(200.0)) {
                if f.px(x, y) != fill {
                    first = Some(x);
                    break;
                }
            }
            let want = left + sc.dev(60.0);
            assert_eq!(
                first,
                Some(want),
                "scale {s} {page}: the hairline's first column is round(60 * scale) in"
            );
            // The tile of the first row: a 32 by 32 box of the tile fill.
            // (the second row, away from the panel's rounded corners; the label column starts at 60)
            let r0 = &sec.rows[1];
            let (y0, y1) = (sc.dev(r0.y) + sc.dev(3.0), sc.dev(r0.y) + sc.dev(50.0));
            let bb = f
                .bbox_where(left + sc.dev(6.0), y0, left + sc.dev(58.0), y1, |p| {
                    p != fill
                })
                .unwrap_or_else(|| panic!("scale {s} {page}: the tile draws"));
            let w = bb.2 - bb.0 + 1;
            let hgt = bb.3 - bb.1 + 1;
            let side = sc.dev(32.0);
            assert!(
                w.abs_diff(side) <= 1 && hgt.abs_diff(side) <= 1,
                "scale {s} {page}: the tile is {w} by {hgt} device px, wanted {side}"
            );
            assert!(
                bb.0.abs_diff(left + sc.dev(14.0)) <= 1,
                "scale {s} {page}: the tile starts 14 in"
            );
        }
    }
}

#[test]
fn nothing_overlaps_and_nothing_scrolls_sideways() {
    for s in SCALES {
        for w in WIDTHS {
            for page in [
                "appearance",
                "terminal",
                "agents",
                "dictation",
                "diagnostics",
                "about",
            ] {
                let sc = Scene::new(page, s, w, 640);
                let pane_w = sc.frame_w - sc.rail_w();
                let eps = 1e-6;
                let mut prev_bottom = sc.layout.header_y + sc.layout.header_h;
                for sec in &sc.layout.sections {
                    assert!(
                        sec.title_y + eps >= prev_bottom,
                        "{page} {w} @{s}: a heading starts below what is above it"
                    );
                    assert!(
                        sec.panel_y + eps >= sec.title_y + sec.title_h,
                        "{page} {w} @{s}: the panel starts below its heading"
                    );
                    assert!(
                        sec.panel_x + sec.panel_w <= pane_w + eps,
                        "{page} {w} @{s}: the panel ends inside the pane ({} of {pane_w})",
                        sec.panel_x + sec.panel_w
                    );
                    let mut edge = sec.panel_y;
                    for r in &sec.rows {
                        assert!(r.y + eps >= edge, "{page} {w} @{s}: rows are disjoint");
                        assert!(
                            r.x + r.w <= sec.panel_x + sec.panel_w + eps
                                && r.x + eps >= sec.panel_x,
                            "{page} {w} @{s}: a row sits inside its panel"
                        );
                        edge = r.y + r.h;
                    }
                    assert!(
                        edge <= sec.panel_y + sec.panel_h + eps,
                        "{page} {w} @{s}: the rows end inside the panel"
                    );
                    prev_bottom = sec.panel_y + sec.panel_h;
                }
                assert!(
                    sc.layout.content_h + eps >= prev_bottom,
                    "{page} {w} @{s}: the content is as tall as its last panel"
                );
                // The drawn frame agrees with the model.
                let comp = &sc.h.comp;
                let want_rail = if w < 760 { 56.0 } else { 244.0 };
                assert_eq!(
                    f64::from(comp.get_rail_w()),
                    want_rail,
                    "{page} {w} @{s}: rail"
                );
                let right = f64::from(comp.get_content_x() + comp.get_content_w());
                assert!(
                    right <= f64::from(comp.get_pane_w()) + eps,
                    "{page} {w} @{s}: nothing is wider than the pane (no sideways scroll)"
                );
                if let Some(first) = sc.layout.sections.first() {
                    assert!(
                        within(f64::from(comp.get_content_x()), first.panel_x, eps),
                        "{page} {w} @{s}: the view and the layout agree on the column"
                    );
                    assert!(
                        within(
                            f64::from(comp.get_content_w()),
                            first.panel_w,
                            1.0 / s as f64 + eps
                        ),
                        "{page} {w} @{s}: the view and the layout agree on the width ({} vs {})",
                        comp.get_content_w(),
                        first.panel_w
                    );
                }
            }
        }
    }
}

#[test]
fn every_edge_is_a_whole_device_pixel() {
    for s in SCALES {
        let sc64 = f64::from(s);
        for w in WIDTHS {
            for page in [
                "appearance",
                "terminal",
                "agents",
                "dictation",
                "diagnostics",
                "about",
            ] {
                let model_scene = Scene::new(page, s, w, 640);
                let l = &model_scene.layout;
                let edge = |what: &str, v: f64| {
                    assert!(
                        whole(v, sc64),
                        "{page} {w} @{s}: {what} = {v} is not on a device pixel"
                    );
                };
                edge("header y", l.header_y);
                edge("header bottom", l.header_y + l.header_h);
                edge("content h", l.content_h);
                for sec in &l.sections {
                    edge("title y", sec.title_y);
                    edge("title bottom", sec.title_y + sec.title_h);
                    edge("panel x", sec.panel_x);
                    edge("panel right", sec.panel_x + sec.panel_w);
                    edge("panel y", sec.panel_y);
                    edge("panel bottom", sec.panel_y + sec.panel_h);
                    let hair_dev = f64::from(model_scene.hair_dev());
                    for (i, r) in sec.rows.iter().enumerate() {
                        edge("row x", r.x);
                        edge("row right", r.x + r.w);
                        edge("row y", r.y);
                        edge("row bottom", r.y + r.h);
                        if let Some(next) = sec.rows.get(i + 1) {
                            assert!(
                                (r.y + r.h - next.y).abs() < 1e-9,
                                "{page} {w} @{s}: neighbours share their edge (no gap, no overlap)"
                            );
                        }
                    }
                    // The first and the last row sit within one device px of the panel's inner edges.
                    if let (Some(first), Some(last)) = (sec.rows.first(), sec.rows.last()) {
                        let top = (first.y - sec.panel_y) * sc64;
                        assert!(
                            top >= hair_dev - 1e-6 && top <= hair_dev + 1.0 + 1e-6,
                            "{page} {w} @{s}: the first row starts {top} device px under the panel"
                        );
                        let bottom = (sec.panel_y + sec.panel_h - (last.y + last.h)) * sc64;
                        assert!(
                            bottom >= hair_dev - 1.0 - 1e-6 && bottom <= hair_dev + 1.0 + 1e-6,
                            "{page} {w} @{s}: the last row ends {bottom} device px above the panel's bottom"
                        );
                    }
                }
                // The rail's boxes.
                let r = &model_scene.rail;
                edge("rail title y", r.title_y);
                edge("rail find y", r.find_y);
                edge("rail find bottom", r.find_y + r.find_h);
                edge("rail inner x", r.inner_x);
                edge("rail inner right", r.inner_x + r.inner_w);
                for (y, h) in &r.items {
                    edge("rail item y", *y);
                    edge("rail item bottom", *y + *h);
                }
                edge("about y", about_y(r, 640.0));
                // The view's own columns.
                let comp = &model_scene.h.comp;
                // The view reports f32 lengths, so a looser tolerance than the f64 layout's.
                for (what, v) in [
                    ("view content x", f64::from(comp.get_content_x())),
                    ("view content top", f64::from(comp.get_content_top())),
                ] {
                    assert!(
                        (v * sc64 - (v * sc64).round()).abs() < 1e-3,
                        "{page} {w} @{s}: {what} = {v} is not on a device pixel"
                    );
                }
            }
        }
    }
}

#[test]
fn the_rail_and_the_find_field_match_prism_terminal() {
    let fx = [("1", 1.0_f32), ("1.5", 1.5_f32)];
    for (key, s) in fx {
        for page in MEASURED {
            let sc = Scene::new(page, s, fx_w(key), 604);
            let r = &sc.rail;
            let nav = pt_box(key, page, "nav");
            assert!(
                within(r.width, nav.2, 1.0),
                "{page} @{key}: rail width {} vs {}",
                r.width,
                nav.2
            );
            let title = pt_box(key, page, "navTitle");
            assert!(
                within(r.title_y, title.1, 1.0),
                "{page} @{key}: title y {} vs {}",
                r.title_y,
                title.1
            );
            assert!(within(r.title_h, title.3, 1.0), "{page} @{key}: title h");
            let find = pt_box(key, page, "find");
            assert!(
                within(r.find_h, find.3, 1.0),
                "{page} @{key}: find is {} high, PT {}",
                r.find_h,
                find.3
            );
            assert!(within(r.inner_x, find.0, 1.0), "{page} @{key}: find x");
            assert!(within(r.inner_w, find.2, 1.0), "{page} @{key}: find width");
            // PT's own `find` box reads y 88.5 in the file, which lies under the first tab; the field is
            // 14 above the first tab (the tab y less the gap less the field's height), which is what
            // the layout holds (Metrics.find_gap).
            let tab = pt_box(key, "appearance", "tab_appearance");
            assert!(
                within(r.find_y, tab.1 - 14.0 - find.3, 1.0),
                "{page} @{key}: the Find field sits 14 above the first tab ({} vs {})",
                r.find_y,
                tab.1 - 14.0 - find.3
            );
            // The items.
            for (i, p) in PAGES.iter().enumerate() {
                // Prism Terminal's file measures each page's own tab as `tab_<page>` on that page.
                let name = format!("tab_{}", p.id);
                if fixture()["scales"][key][page].get(&name).is_none() {
                    continue;
                }
                let t = pt_box(key, page, &name);
                let y = if p.end {
                    about_y(r, 604.0)
                } else {
                    r.items[i].0
                };
                assert!(
                    within(y, t.1, 1.0),
                    "{page} @{key}: tab {} y {y} vs {}",
                    p.id,
                    t.1
                );
                assert!(
                    within(r.items[i].1, t.3, 1.0),
                    "{page} @{key}: tab {} height",
                    p.id
                );
                assert!(
                    within(r.inner_w, t.2, 1.0),
                    "{page} @{key}: tab {} width",
                    p.id
                );
            }
        }
    }
    // The narrow rail is 56 below 760 css px.
    assert_eq!(rail_layout(759.0, 1.0, 6).width, 56.0);
    assert_eq!(rail_layout(760.0, 1.0, 6).width, 244.0);
}

#[test]
fn the_rows_and_controls_match_prism_terminals_boxes() {
    // (page, PT selector, Flare path as (width, height) in css px). The Flare path is the component's own
    // out geometry in its Preview window, or the SettingsShape token the component draws with.
    let css_h = |v: f32| f64::from(v);
    for (key, s) in [("1", 1.0_f32), ("1.5", 1.5_f32)] {
        // The row and the tile: from the real layout and tokens.
        for page in ["appearance", "terminal", "agents"] {
            let sc = Scene::new(page, s, fx_w(key), 604);
            let (si, ri) = row_blocks(&sc.layout)[0];
            let r = &sc.layout.sections[si].rows[ri];
            let row = pt_box(key, page, "row");
            assert!(
                within(r.h, row.3, 1.0),
                "{page} @{key}: row h {} vs {}",
                r.h,
                row.3
            );
            // The row is drawn inside the panel's border, which is a device pixel wide (Chromium's rule):
            // 0.67 css px at 150 percent, so the row Slint draws is the panel less two of those.
            let sec = &sc.layout.sections[si];
            let drawn_w = sec.panel_w - 2.0 * sc.hair();
            assert!(
                within(drawn_w, row.2, 1.0),
                "{page} @{key}: row w {} vs {}",
                drawn_w,
                row.2
            );
            let tile = pt_box(key, page, "rowTile");
            assert!(
                within(shape("tile"), tile.2, 1.0) && within(shape("tile"), tile.3, 1.0),
                "{page} @{key}: tile"
            );
        }
        // The switch, with its knob (Terminal and Agents).
        for page in ["terminal", "agents"] {
            let (_, _, w, h) = pt_box(key, page, "switchOne");
            let hn = Harness::new(s, 400, 200, || ControlsPreview::new().expect("controls"));
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_sw_w()), w, 1.0),
                "{page} @{key}: switch width"
            );
            assert!(
                within(css_h(hn.comp.get_sw_h()), h, 1.0),
                "{page} @{key}: switch height"
            );
        }
        let knob = pt_box(key, "terminal", "switchKnob");
        assert!(within(shape("switch-knob"), knob.2, 1.0), "@{key}: knob");
        // The select trigger: 32 high, 168 wide at least.
        let trig = pt_box(key, "terminal", "selectTrigger");
        {
            let hn = Harness::new(s, 400, 420, || SelectPreview::new().expect("select"));
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_trig_h()), trig.3, 1.0),
                "@{key}: trigger height"
            );
            assert!(
                css_h(hn.comp.get_trig_w()) >= trig.2 - 1.0,
                "@{key}: trigger width at least 168"
            );
            assert!(
                within(shape("select-min-w"), trig.2, 1.0),
                "@{key}: select-min-w"
            );
        }
        // The segmented pill and the Save button: heights (the widths follow the text).
        {
            // PT's `segmented` is one segment (17.25 of text and 4 above and below: 25.25 high); the
            // pill round it is `rowControl` (1 border, 3 padding: 33.25 at 100 percent, 32.58 at 150).
            // Flare fixes the pill at 32 and the segment at 24, so at 100 percent they are 1.25 short:
            // a documented difference (listed in docs/screenshots/m7/README.md by Task 53).
            let seg = pt_box(key, "appearance", "segmented");
            let pill = pt_box(key, "appearance", "rowControl");
            let save = pt_box(key, "appearance", "saveTerm");
            const SEGMENT_DIFF: f64 = 1.3;
            // The Save button: PT is 17.25 of text (line height 1.5) + 8 + a 2 px border = 27.25; Flare
            // sizes it to the 16 px line of its text, 3.25 short. Also listed for Task 53.
            const SAVE_DIFF: f64 = 3.3;
            let hn = Harness::new(s, 400, 200, || ControlsPreview::new().expect("controls"));
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_seg_h()), pill.3, SEGMENT_DIFF),
                "@{key}: segmented pill height {} vs {}",
                hn.comp.get_seg_h(),
                pill.3
            );
            assert!(
                within(24.0, seg.3, SEGMENT_DIFF),
                "@{key}: the segment is 24 high, PT's {}",
                seg.3
            );
            assert!(
                within(css_h(hn.comp.get_save_h()), save.3, SAVE_DIFF),
                "@{key}: Save height {} vs {}",
                hn.comp.get_save_h(),
                save.3
            );
        }
        // The slider track: 168 by 6.
        {
            let slider = pt_box(key, "appearance", "slider");
            assert!(
                within(shape("slider-w"), slider.2, 1.0)
                    && within(shape("slider-track-h"), slider.3, 1.0),
                "@{key}: slider tokens"
            );
            let hn = Harness::new(s, 260, 40, || SliderPreview::new().expect("slider"));
            hn.comp.set_text("30%".into());
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_tr_h()), slider.3, 1.0),
                "@{key}: slider track height {}",
                hn.comp.get_tr_h()
            );
            assert!(
                within(css_h(hn.comp.get_tr_w()), slider.2, 1.0),
                "@{key}: slider track width {}",
                hn.comp.get_tr_w()
            );
        }
        // The colour field: the swatch 36 by 24, the code 76 or 172 wide.
        {
            let sw = pt_box(key, "appearance", "colourSwatch");
            let hn = Harness::new(s, 520, 440, || ColourPreview::new().expect("colour"));
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_swatch_w()), sw.2, 1.0),
                "@{key}: swatch width {}",
                hn.comp.get_swatch_w()
            );
            assert!(
                within(css_h(hn.comp.get_swatch_h()), sw.3, 1.0),
                "@{key}: swatch height {}",
                hn.comp.get_swatch_h()
            );
            assert!(
                within(css_h(hn.comp.get_code_w()), 76.0, 1.0),
                "@{key}: HEX code field is 76 wide"
            );
            hn.comp.set_field_wide(true);
            hn.advance(0);
            assert!(
                within(css_h(hn.comp.get_code_w()), 172.0, 1.0),
                "@{key}: other code fields are 172 wide"
            );
        }
    }
}

#[test]
fn the_section_and_header_rhythm_matches() {
    for (key, s) in [("1", 1.0_f32), ("1.5", 1.5_f32)] {
        for page in MEASURED {
            let sc = Scene::new(page, s, fx_w(key), 604);
            let l = &sc.layout;
            let header = pt_box(key, page, "header");
            assert!(
                within(l.header_y, header.1, 1.0),
                "{page} @{key}: header y {} vs {}",
                l.header_y,
                header.1
            );
            assert!(
                within(sc.rail_w() + l.pad_x, header.0, 1.0),
                "{page} @{key}: header x"
            );
            let panel = pt_box(key, page, "panel");
            let first = &l.sections[0];
            assert!(
                within(sc.rail_w() + first.panel_x, panel.0, 1.0),
                "{page} @{key}: panel x"
            );
            assert!(
                within(first.panel_w, panel.2, 1.0),
                "{page} @{key}: panel width {} vs {}",
                first.panel_w,
                panel.2
            );
            // (Flare's About panel also holds the "Made with Slint" badge, decision O9; Prism Terminal's
            // has no such block, so its height is compared without it.)
            let badge: f64 = first
                .rows
                .iter()
                .filter(|r| r.block == LayoutBlock::Slint)
                .map(|r| r.h)
                .sum();
            assert!(
                within(first.panel_h - badge, panel.3, 1.0),
                "{page} @{key}: first panel h {} vs {}",
                first.panel_h - badge,
                panel.3
            );
            // The sections that have a heading: 18 under the header, 8 above the panel, 26 apart.
            let titled = |i: usize| !sc.model.sections[i].title.is_empty();
            if titled(0) {
                let heading = pt_box(key, page, "sectionHeading");
                assert!(
                    within(first.title_y, heading.1, 1.0),
                    "{page} @{key}: first heading y {} vs {}",
                    first.title_y,
                    heading.1
                );
                assert!(
                    within(first.panel_y, panel.1, 1.0),
                    "{page} @{key}: first panel y {} vs {}",
                    first.panel_y,
                    panel.1
                );
                assert!(
                    within(first.title_y - (l.header_y + l.header_h), 18.0, 1.0),
                    "{page} @{key}: the first heading is 18 under the header"
                );
            }
            for (i, pair) in l.sections.windows(2).enumerate() {
                if !titled(i + 1) {
                    continue;
                }
                let gap = pair[1].title_y - (pair[0].panel_y + pair[0].panel_h);
                assert!(
                    within(gap, 26.0, 1.0),
                    "{page} @{key}: 26 between sections, got {gap}"
                );
            }
            for (i, sec) in l.sections.iter().enumerate() {
                if !titled(i) {
                    continue;
                }
                let gap = sec.panel_y - (sec.title_y + sec.title_h);
                assert!(
                    within(gap, 8.0, 1.0),
                    "{page} @{key}: 8 between a heading and its panel, got {gap}"
                );
            }
        }
    }
}

/// Prism Terminal draws no heading for a section without a title (Diagnostics, About, the first panel of
/// Dictation) and puts its panel 18 under the header. `layout_page` reserves a heading's height for such
/// a section anyway, so the panel sits 27.5 lower than Prism Terminal's.
#[test]
fn a_section_without_a_title_reserves_no_heading_gap() {
    for (key, s) in [("1", 1.0_f32), ("1.5", 1.5_f32)] {
        for page in ["diagnostics", "about"] {
            let sc = Scene::new(page, s, fx_w(key), 604);
            assert!(
                sc.model.sections[0].title.is_empty(),
                "{page}: the section has no title"
            );
            let panel = pt_box(key, page, "panel");
            let first = &sc.layout.sections[0];
            assert!(
                within(first.panel_y, panel.1, 1.0),
                "{page} @{key}: the first panel is at y {}, the reference's at {}",
                first.panel_y,
                panel.1
            );
        }
    }
}
