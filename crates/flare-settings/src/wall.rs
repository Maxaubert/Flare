//! The theme wall model: order, card data and layout (M7 Task 17).
//! Source: PT `core/renderer/settings/theme/ThemeWall.tsx` (the wall, `TermThemeCard`, `TWO_ROWS`) and
//! `palette.ts` (`cardAnsi`); the order is M4's `flare_theme::order` (PT `themeOrder.ts`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use flare_theme::look::card_ansi;
use flare_theme::order::wall_ids;
use flare_theme::preset::{HOST_DEFAULT_THEME, Preset, find_preset, presets};
use flare_theme::resolve::{TermTheme, resolve_custom_theme, resolve_preset};

use crate::look::LookStore;

/// Every preset id that `preset_look` has resolved, in order (tests only).
#[cfg(test)]
static RESOLVES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// A preset's resolved look, worked out once per preset (PT `palette.ts` `presetLook`, owner 2026-09-22):
/// a preset never changes, and a resolve runs the legibility floors over its sixteen colours, so the wall
/// must not pay that for every card on every change to the page.
pub fn preset_look(p: &Preset) -> TermTheme {
    static LOOKS: OnceLock<Mutex<HashMap<&'static str, TermTheme>>> = OnceLock::new();
    let looks = LOOKS.get_or_init(Default::default);
    let mut looks = looks.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = looks.get(p.id) {
        return t.clone();
    }
    #[cfg(test)]
    RESOLVES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(p.id);
    let t = resolve_preset(p);
    looks.insert(p.id, t.clone());
    t
}

/// PT's `grid-template-columns: repeat(auto-fill, minmax(172px, 1fr))` (PT PR #251, owner 2026-10-21):
/// a card is never narrower than this, and the cards of a row stretch to fill it.
pub const MIN_CARD_W: f64 = 172.0;
/// The width a card is drawn at where no wall lays it out (the editor's preview).
pub const CARD_W: f64 = 196.0;
pub const GAP: f64 = 12.0;
pub const TWO_ROWS: f64 = 268.0;
/// Top, right, bottom, left.
pub const WALL_PAD: (f64, f64, f64, f64) = (14.0, 16.0, 16.0, 16.0);
/// Task 3 measures PT's card; this is that number (the test reads `pt_settings_metrics.json`).
pub const CARD_H: f64 = 126.0;
pub const PREVIEW_H: f64 = 92.0;
pub const LABEL_H: f64 = 32.0;
/// The Show all button, width and height.
pub const TOGGLE: (f64, f64) = (40.0, 28.0);
pub const EXPAND_MS: u64 = 240;

/// One card of the wall, as drawn: the theme's own colours, never the chrome's.
#[derive(Clone, Debug, PartialEq)]
pub struct CardData {
    pub id: String,
    pub name: String,
    pub on: bool,
    pub edit: bool,
    pub bg: String,
    pub fg: String,
    pub cursor: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub cyan: String,
    pub red: String,
}

/// Where each card sits in a wall of a given width, and how tall the clip is.
#[derive(Clone, Debug, PartialEq)]
pub struct WallLayout {
    pub cols: usize,
    pub rows: usize,
    pub positions: Vec<(f64, f64)>,
    /// Each card's width: whole device pixel edges, so a card may differ from its neighbour by one.
    pub widths: Vec<f64>,
    pub full_h: f64,
    pub clip_h: f64,
}

fn card(id: String, name: &str, on: bool, t: &TermTheme) -> CardData {
    let a = card_ansi(t);
    CardData {
        id,
        name: name.to_string(),
        on,
        edit: on,
        bg: t.background.clone(),
        fg: t.foreground.clone(),
        cursor: t.cursor.clone(),
        green: a.green,
        yellow: a.yellow,
        blue: a.blue,
        cyan: a.cyan,
        red: a.red,
    }
}

/// The cards in wall order: Custom first when one is saved, then the host default, then the rest. The
/// Custom card is drawn by its resolved colours; `on` marks the theme in force and only it wears the pencil.
pub fn wall_cards(look: &LookStore) -> Vec<CardData> {
    let in_force = look.theme_id();
    let custom = look.custom();
    wall_ids(custom.is_some(), HOST_DEFAULT_THEME)
        .into_iter()
        .filter_map(|id| {
            let on = id == in_force;
            if id == "custom" {
                let c = custom.as_ref()?;
                let t = resolve_custom_theme(&c.palette, None);
                return Some(card(id, "Custom", on, &t));
            }
            let p = find_preset(&id)?;
            let t = preset_look(p);
            Some(card(id, p.name, on, &t))
        })
        .collect()
}

/// Columns of the wall and the placement of `n` cards. Collapsed, the clip is two rows; expanded, all of it.
/// `width` is the wall's css width and `scale` the device pixels per css pixel. The columns are PT's
/// `repeat(auto-fill, minmax(172px, 1fr))` with a 12 px gap: floor((width + 12) / (172 + 12)) of them, each
/// card stretched to (width - gaps) / cols, and every edge snapped to a whole device pixel.
pub fn layout(width: f64, n: usize, expanded: bool, scale: f64) -> WallLayout {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let snap = |v: f64| (v * k).round() / k;
    let cols = ((width + GAP) / (MIN_CARD_W + GAP)).floor().max(1.0) as usize;
    let card_w = ((width - (cols - 1) as f64 * GAP) / cols as f64).max(MIN_CARD_W);
    let mut positions = Vec::with_capacity(n);
    let mut widths = Vec::with_capacity(n);
    for i in 0..n {
        let (row, col) = (i / cols, i % cols);
        let left = col as f64 * (card_w + GAP);
        // the right edge rounds DOWN, so the last card never reaches past the wall (a strip of 0 to 1 device px)
        let right = ((left + card_w) * k + 1e-6).floor() / k;
        let x = snap(left);
        positions.push((x, row as f64 * (CARD_H + GAP)));
        widths.push(right - x);
    }
    let rows = n.div_ceil(cols);
    let full_h = if rows == 0 {
        0.0
    } else {
        rows as f64 * CARD_H + (rows - 1) as f64 * GAP
    };
    let clip_h = if expanded {
        full_h
    } else {
        full_h.min(TWO_ROWS)
    };
    WallLayout {
        cols,
        rows,
        positions,
        widths,
        full_h,
        clip_h,
    }
}

/// Progress on cubic-bezier(.16, 1, .3, 1) at time fraction `x`: the bezier's curve is solved for its
/// parameter by bisection (x is monotonic on these control points).
fn ease_progress(x: f64) -> f64 {
    const X1: f64 = 0.16;
    const Y1: f64 = 1.0;
    const X2: f64 = 0.3;
    const Y2: f64 = 1.0;
    let bx =
        |s: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * X1 + 3.0 * (1.0 - s) * s * s * X2 + s * s * s;
    let by =
        |s: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * Y1 + 3.0 * (1.0 - s) * s * s * Y2 + s * s * s;
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if bx(mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    by((lo + hi) / 2.0)
}

/// The clip's height `t_ms` into the expand, on cubic-bezier(.16, 1, .3, 1) over `EXPAND_MS`. Collapsing
/// (`to` at or below `from`) is instant and returns `to`.
pub fn expand_height(t_ms: u64, from: f64, to: f64) -> f64 {
    if to <= from {
        return to;
    }
    if t_ms == 0 {
        return from;
    }
    if t_ms >= EXPAND_MS {
        return to;
    }
    let x = t_ms as f64 / EXPAND_MS as f64;
    from + (to - from) * ease_progress(x)
}

/// (aria label, title) of the Show all button.
pub fn toggle_texts(expanded: bool, total: usize) -> (String, String) {
    if expanded {
        ("Show fewer themes".to_string(), "Show fewer".to_string())
    } else {
        (
            format!("Show all {total} themes"),
            "Show all themes".to_string(),
        )
    }
}

/// (aria label, title) of the pencil on the selected card.
pub fn edit_texts(name: &str) -> (String, String) {
    (
        format!("Edit {name}"),
        "Edit colours, saved as Custom".to_string(),
    )
}

/// Presets plus one when a Custom is saved.
pub fn total_themes(custom_saved: bool) -> usize {
    presets().len() + usize::from(custom_saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_theme::CustomTheme;
    use crate::kv::PrefStore;
    use flare_theme::resolve::CustomPalette;

    fn look() -> LookStore {
        LookStore::new(PrefStore::memory())
    }

    fn palette(bg: &str) -> CustomPalette {
        CustomPalette {
            bg: bg.to_string(),
            fg: "#e0e0e0".to_string(),
            cursor: "#ffcc00".to_string(),
            selection: None,
            ansi: Default::default(),
            accent: None,
        }
    }

    fn save(look: &LookStore, p: CustomPalette) {
        look.save_custom(&CustomTheme {
            palette: p,
            indicator: None,
            indicator_color: None,
            done_color: None,
            question_color: None,
            see_through: None,
        });
    }

    fn ids(cards: &[CardData]) -> Vec<&str> {
        cards.iter().map(|c| c.id.as_str()).collect()
    }

    fn metrics() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("flare")
            .join("tests")
            .join("fixtures")
            .join("pt_settings_metrics.json");
        let text = std::fs::read_to_string(&path).expect("the metrics fixture");
        serde_json::from_str(&text).expect("valid JSON")
    }

    fn theme_order_fixture() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("flare-theme")
            .join("tests")
            .join("fixtures")
            .join("wall_order.json");
        let text = std::fs::read_to_string(&path).expect("the wall order fixture");
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        v.as_array()
            .expect("an array")
            .iter()
            .map(|x| x.as_str().expect("an id").to_string())
            .collect()
    }

    #[test]
    fn custom_leads_then_pt_default_then_the_ordered_rest() {
        let l = look();
        let without: Vec<String> = wall_cards(&l).into_iter().map(|c| c.id).collect();
        assert_eq!(without[0], "pt-default");
        assert_eq!(without.len(), 39);
        let rest_want: Vec<String> = theme_order_fixture()
            .into_iter()
            .filter(|id| id != "pt-default")
            .collect();
        assert_eq!(without[1..].to_vec(), rest_want);

        save(&l, palette("#101010"));
        let with = wall_cards(&l);
        assert_eq!(&ids(&with)[..2], ["custom", "pt-default"]);
        assert_eq!(with.len(), 40);
        let rest: Vec<String> = with[2..].iter().map(|c| c.id.clone()).collect();
        assert_eq!(rest, without[1..].to_vec());
    }

    #[test]
    fn the_card_of_the_theme_in_force_is_on_and_only_it_has_the_pencil() {
        let l = look();
        l.set_theme_id("monokai");
        let cards = wall_cards(&l);
        let on: Vec<&CardData> = cards.iter().filter(|c| c.on).collect();
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].id, "monokai");
        assert!(on[0].edit);
        assert!(cards.iter().filter(|c| c.edit).all(|c| c.on));
    }

    #[test]
    fn a_custom_card_draws_what_the_terminal_draws() {
        let l = look();
        let mut p = palette("#101010");
        p.cursor = "#ffcc0080".to_string();
        save(&l, p.clone());
        let cards = wall_cards(&l);
        let custom = cards
            .iter()
            .find(|c| c.id == "custom")
            .expect("a Custom card");
        let drawn = resolve_custom_theme(&p, None);
        assert_eq!(custom.cursor, drawn.cursor);
        assert_eq!(
            custom.cursor.len(),
            7,
            "the cursor is the six digit resolved one"
        );
        assert_eq!(custom.name, "Custom");
    }

    #[test]
    fn a_card_falls_back_per_colour() {
        let l = look();
        let mut p = palette("#101010");
        p.ansi[2] = Some("#00ff00".to_string());
        save(&l, p);
        let custom = wall_cards(&l)
            .into_iter()
            .find(|c| c.id == "custom")
            .expect("a Custom card");
        assert_eq!(custom.green, "#00ff00");
        assert_eq!(custom.yellow, "#d1a54b");
        assert_eq!(custom.blue, "#4aa5f0");
        assert_eq!(custom.cyan, "#42b3c2");
        assert_eq!(custom.red, "#e05561");
    }

    #[test]
    fn the_card_palette_is_computed_once_per_preset() {
        let l = look();
        let first = wall_cards(&l);
        let second = wall_cards(&l);
        assert_eq!(first, second, "a second read gives the same cards");
        let resolves = RESOLVES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for p in presets() {
            let n = resolves.iter().filter(|id| **id == p.id).count();
            assert!(n <= 1, "preset {} was resolved {n} times", p.id);
        }
        for c in first.iter().filter(|c| c.id != "custom") {
            let n = resolves.iter().filter(|id| **id == c.id.as_str()).count();
            assert_eq!(n, 1, "the card {} is resolved once, not {n} times", c.id);
        }
    }

    #[test]
    fn layout_wraps_at_the_column_count() {
        let cases = [
            (172.0, 1, 5370.0),
            (355.0, 1, 5370.0),
            (356.0, 2, 2748.0),
            (540.0, 3, 1782.0),
            (724.0, 4, 1368.0),
            (1000.0, 5, 1092.0),
        ];
        for (width, cols, full_h) in cases {
            let w = layout(width, 39, true, 1.0);
            assert_eq!(w.cols, cols, "width {width}");
            assert_eq!(w.positions.len(), 39);
            assert_eq!(w.widths.len(), 39);
            assert!(
                (w.full_h - full_h).abs() < 1e-9,
                "width {width}: {}",
                w.full_h
            );
        }
        let four = layout(724.0, 39, true, 1.0);
        assert_eq!(four.positions[0], (0.0, 0.0));
        assert_eq!(four.widths[0], 172.0);
        assert_eq!(four.positions[3], (3.0 * (MIN_CARD_W + GAP), 0.0));
        assert_eq!(four.positions[4], (0.0, CARD_H + GAP));
        assert_eq!(layout(0.0, 39, true, 1.0).cols, 1);
    }

    #[test]
    fn a_width_that_fits_four_at_172_gives_four_and_one_less_gives_three() {
        let fits = 4.0 * MIN_CARD_W + 3.0 * GAP;
        for k in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            assert_eq!(layout(fits, 39, false, k).cols, 4, "@{k}");
            assert_eq!(layout(fits - 0.01, 39, false, k).cols, 3, "@{k}");
        }
    }

    /// PT's e2e measures the strip right of the last card of a row: 0 to 2 css px. Every width, every scale,
    /// and every edge is a whole device pixel.
    #[test]
    fn the_cards_fill_the_row_on_whole_device_pixels_at_every_width_and_scale() {
        for k in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let mut width = 172.0;
            while width <= 1500.0 {
                let l = layout(width, 39, true, k);
                assert_eq!(
                    l.cols,
                    (((width + GAP) / (MIN_CARD_W + GAP)).floor() as usize).max(1)
                );
                let on_grid = |v: f64| ((v * k).round() - v * k).abs() < 1e-6;
                for i in 0..l.cols.min(39) {
                    let (x, _) = l.positions[i];
                    let w = l.widths[i];
                    assert!(on_grid(x) && on_grid(x + w), "@{k} width {width}: card {i}");
                    assert!(
                        w >= MIN_CARD_W - 1.0 / k - 1e-9,
                        "@{k} width {width}: {w} wide"
                    );
                    if i + 1 < l.cols {
                        let gap = l.positions[i + 1].0 - (x + w);
                        assert!(
                            (gap - GAP).abs() <= 1.0 / k + 1e-9,
                            "@{k} width {width}: gap {gap}"
                        );
                    }
                }
                let last = l.cols - 1;
                let strip = width - (l.positions[last].0 + l.widths[last]);
                assert!(
                    (-1e-9..=2.0).contains(&strip),
                    "@{k} width {width}: {strip} css px right of the last card"
                );
                width += 7.3;
            }
        }
    }

    #[test]
    fn a_partial_last_row_keeps_the_same_columns() {
        let l = layout(724.0, 6, true, 2.25);
        assert_eq!((l.cols, l.rows), (4, 2));
        assert_eq!(l.positions[4].0, l.positions[0].0);
        assert_eq!(l.widths[4], l.widths[0]);
    }

    #[test]
    fn collapsed_clip_is_268_and_expanded_is_the_full_height() {
        let collapsed = layout(724.0, 39, false, 1.0);
        assert_eq!(collapsed.clip_h, TWO_ROWS);
        let expanded = layout(724.0, 39, true, 1.0);
        assert_eq!(expanded.clip_h, expanded.full_h);
        assert!((expanded.full_h - 1368.0).abs() < 1e-9);
    }

    #[test]
    fn a_card_is_126_high_like_the_reference() {
        let m = metrics();
        let h = m["scales"]["1"]["appearance"]["termCard"]["height"]
            .as_f64()
            .expect("the termCard height at scale 1");
        assert!((CARD_H - h).abs() <= 1.0, "CARD_H {CARD_H} vs PT {h}");
    }

    #[test]
    fn expand_eases_open_and_snaps_shut() {
        assert_eq!(expand_height(0, 268.0, 1100.0), 268.0);
        assert_eq!(expand_height(EXPAND_MS, 268.0, 1100.0), 1100.0);
        let mid = expand_height(120, 268.0, 1100.0);
        assert!(mid > 268.0 && mid < 1100.0, "mid {mid}");
        assert!(expand_height(60, 268.0, 1100.0) > 268.0 + (1100.0 - 268.0) * 0.5 * 0.5);
        assert_eq!(expand_height(0, 1100.0, 268.0), 268.0);
        assert_eq!(expand_height(100, 1100.0, 268.0), 268.0);
    }

    #[test]
    fn toggle_and_edit_texts() {
        assert_eq!(
            toggle_texts(false, 40),
            (
                "Show all 40 themes".to_string(),
                "Show all themes".to_string()
            )
        );
        assert_eq!(
            toggle_texts(true, 40),
            ("Show fewer themes".to_string(), "Show fewer".to_string())
        );
        assert_eq!(
            edit_texts("Umber"),
            (
                "Edit Umber".to_string(),
                "Edit colours, saved as Custom".to_string()
            )
        );
        assert_eq!(total_themes(false), 39);
        assert_eq!(total_themes(true), 40);
    }
}
