//! UI tests of the theme wall, the editor and the switch question (M7 Task 37); headless only.
//!
//! Every test runs on the headless harness (no window). Box geometry comes from the preview's out
//! properties (logical px); colours are read from the rendered frame. Keys and presses go through
//! the harness; nothing here reaches the real clipboard, registry or a visible window.

use slint::platform::Key;
use slint::{Color, ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{EditorWell, WallCard, WallPreview};

/// `Theme.accent-hi`, `Theme.accent` and `Theme.ground` (the pt-default values).
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const GROUND: [u8; 4] = [23, 23, 23, 255];
const CHROME_TEXT: [u8; 4] = [0xde, 0xda, 0xcf, 255];

/// A card palette that shares no colour with the chrome.
const BG: [u8; 4] = [16, 24, 32, 255];
const FG: [u8; 4] = [190, 190, 255, 255];
const CURSOR: [u8; 4] = [255, 0, 200, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const BLUE: [u8; 4] = [60, 120, 255, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];
const RED: [u8; 4] = [255, 40, 40, 255];

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

const CSS_W: u32 = 1000;
const CSS_H: u32 = 640;

fn colour(c: [u8; 4]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

fn key(k: Key) -> String {
    char::from(k).to_string()
}

fn card(id: &str, name: &str, on: bool, edit: bool, x: f32, y: f32) -> WallCard {
    WallCard {
        id: id.into(),
        name: name.into(),
        on,
        edit,
        edit_name: format!("Edit {name}").into(),
        bg: colour(BG),
        fg: colour(FG),
        cursor: colour(CURSOR),
        green: colour(GREEN),
        yellow: colour(YELLOW),
        blue: colour(BLUE),
        cyan: colour(CYAN),
        red: colour(RED),
        x,
        y,
    }
}

/// The ANSI and base names of the editor's twenty wells, in the order the plan draws them.
fn well_names() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("bg", "Background"),
        ("fg", "Foreground"),
        ("cursor", "Cursor"),
        ("selection", "Selection"),
    ];
    for n in [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ] {
        v.push((n, n));
    }
    for n in [
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
    ] {
        v.push((n, n));
    }
    v
}

fn wells(wide: bool, open_at: Option<usize>) -> ModelRc<EditorWell> {
    let v: Vec<EditorWell> = well_names()
        .into_iter()
        .enumerate()
        .map(|(i, (k, label))| EditorWell {
            key: k.into(),
            label: label.into(),
            text: format!("#{:02x}{:02x}{:02x}", i * 10, 40, 200 - i * 5).into(),
            colour: Color::from_rgb_u8((i * 10) as u8, 40, (200 - i * 5) as u8),
            wide,
            open: open_at == Some(i),
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

fn cards_model(cards: Vec<WallCard>) -> ModelRc<WallCard> {
    ModelRc::new(VecModel::from(cards))
}

fn harness(scale: f32) -> Harness<WallPreview> {
    let h = Harness::new(scale, CSS_W, CSS_H, || WallPreview::new().unwrap());
    h.comp.set_wells(wells(false, None));
    h.comp
        .set_preview(card("custom-preview", "Custom", true, false, 0.0, 0.0));
    h.advance(0);
    h
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// The device-pixel height of a hairline at `scale`, in logical px.
fn hair(scale: f32) -> f32 {
    scale.floor().max(1.0) / scale
}

fn near(p: [u8; 4], want: [u8; 4], tol: u8) -> bool {
    Frame::near(p, want, tol)
}

fn is_accent(p: [u8; 4]) -> bool {
    near(p, ACCENT_HI, 8) || near(p, ACCENT, 8)
}

fn metrics() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/pt_settings_metrics.json"
    ))
    .unwrap()
}

/// Three cards in the wall, the middle one chosen and editable.
fn three_cards() -> ModelRc<WallCard> {
    cards_model(vec![
        card("a", "Alpha", false, true, 0.0, 0.0),
        card("b", "Beta", true, true, 208.0, 0.0),
        card("c", "Gamma", false, true, 416.0, 0.0),
    ])
}

fn center(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

#[test]
fn cards_are_196_wide_and_one_height_at_every_scale() {
    let m = metrics();
    for scale in SCALES {
        for chosen in [false, true] {
            let h = harness(scale);
            h.comp.set_solo(card("s", "Solo", chosen, true, 0.0, 0.0));
            h.advance(0);
            assert!(
                close(h.comp.get_solo_w(), 196.0, 0.01),
                "width {} at {scale}",
                h.comp.get_solo_w()
            );
            let want = 124.0 + 2.0 * hair(scale);
            assert!(
                close(h.comp.get_solo_h(), want, 0.01),
                "height {} want {want} at {scale} (chosen {chosen})",
                h.comp.get_solo_h()
            );
            assert!(close(h.comp.get_solo_label_h(), 32.0, 0.01));
        }
    }
    // Prism Terminal's own numbers, within one CSS px
    for (key, scale) in [("1", 1.0_f32), ("1.5", 1.5)] {
        let p = &m["scales"][key]["appearance"];
        let h = harness(scale);
        h.comp.set_solo(card("s", "Solo", true, true, 0.0, 0.0));
        h.advance(0);
        let num = |a: &str, b: &str| p[a][b].as_f64().unwrap() as f32;
        assert!(
            close(h.comp.get_solo_w(), num("termCard", "width"), 1.0),
            "card width at {scale}"
        );
        assert!(
            close(h.comp.get_solo_h(), num("termCard", "height"), 1.0),
            "card height {} vs {} at {scale}",
            h.comp.get_solo_h(),
            num("termCard", "height")
        );
        assert!(
            close(
                h.comp.get_solo_label_h(),
                num("termCardLabel", "height"),
                1.0
            ),
            "label height at {scale}"
        );
    }
}

#[test]
fn the_pencil_is_20_by_20_at_bottom_6_right_10() {
    for scale in SCALES {
        let h = harness(scale);
        h.comp.set_solo(card("s", "Solo", true, true, 0.0, 0.0));
        h.advance(0);
        let (sx, sy, sw, sh) = (
            h.comp.get_solo_x(),
            h.comp.get_solo_y(),
            h.comp.get_solo_w(),
            h.comp.get_solo_h(),
        );
        assert!(close(h.comp.get_solo_pencil_w(), 20.0, 0.01));
        assert!(close(h.comp.get_solo_pencil_h(), 20.0, 0.01));
        let right = sx + sw - (h.comp.get_solo_pencil_x() + 20.0);
        let bottom = sy + sh - (h.comp.get_solo_pencil_y() + 20.0);
        assert!(close(right, 10.0, 0.5), "right gap {right} at {scale}");
        assert!(close(bottom, 6.0, 0.5), "bottom gap {bottom} at {scale}");
    }
}

#[test]
fn the_chosen_card_has_the_accent_edge_and_the_pencil() {
    let scale = 3.0;
    let h = harness(scale);
    h.comp.set_cards(three_cards());
    h.advance(0);
    h.move_to(990.0, 630.0);
    let f = h.render();
    let wx = h.comp.get_wall_x();
    let wy = h.comp.get_wall_y();
    let dev = |v: f32| (v * scale) as u32;
    let boxes: Vec<(u32, u32, u32, u32)> = [0.0_f32, 208.0, 416.0]
        .iter()
        .map(|cx| (dev(wx + cx), dev(wy), dev(wx + cx + 196.0), dev(wy + 126.0)))
        .collect();
    // the others never wear the accent
    for i in [0usize, 2] {
        let (x0, y0, x1, y1) = boxes[i];
        assert_eq!(
            f.count_where(x0, y0, x1, y1, is_accent),
            0,
            "card {i} is not chosen"
        );
    }
    let (x0, y0, x1, y1) = boxes[1];
    // the edge
    assert!(
        near(f.px((x0 + x1) / 2, y0 + 1), ACCENT_HI, 8),
        "top edge is accent-hi, got {:?}",
        f.px((x0 + x1) / 2, y0 + 1)
    );
    // the preview area (inside the border) has none of it
    if let Some(b) = f.bbox_where(x0 + 20, y0 + 20, x1 - 20, y0 + 92 * 3, is_accent) {
        panic!("accent in the preview at {b:?}, card box {:?}", boxes[1]);
    }
    // the label rule is accent-hi at 40 percent: orange-ish, not the grey line
    let rule_y = y0 + 93 * 3 + 1;
    let r = f.px((x0 + x1) / 2, rule_y);
    assert!(
        r[0] >= 90 && i32::from(r[0]) - i32::from(r[2]) >= 30,
        "the label rule of the chosen card is tinted, got {r:?}"
    );
    let r2 = f.px((boxes[0].0 + boxes[0].2) / 2, boxes[0].1 + 93 * 3 + 1);
    assert!(
        i32::from(r2[0]) - i32::from(r2[2]) < 12,
        "the label rule of another card is grey, got {r2:?}"
    );
    // the label text
    let label_px = f.count_where(x0 + 30, y0 + 96 * 3, x0 + 120 * 3, y1 - 4, is_accent);
    assert!(label_px > 20, "the chosen name is accent-hi ({label_px})");
    // the pencil
    let px0 = dev(wx + 208.0 + 166.0);
    let py0 = dev(wy + 126.0 - 26.0);
    let pencil_px = f.count_where(px0, py0, px0 + 60, py0 + 60, is_accent);
    assert!(pencil_px > 10, "the pencil is accent-hi ({pencil_px})");
    // the outer ring: one pixel outside the chosen card, tinted, and clear outside the others
    let ring = f.px(x0 - 2, (y0 + y1) / 2);
    assert!(
        ring[0] > ring[2] && ring[0] > GROUND[0] + 30,
        "the ring is the accent at 45 percent, got {ring:?}"
    );
    let plain = f.px(boxes[2].0 - 2, (boxes[2].1 + boxes[2].3) / 2);
    assert!(
        near(plain, GROUND, 2),
        "no ring beside a plain card, got {plain:?}"
    );
}

#[test]
fn the_pencil_only_appears_on_the_chosen_card() {
    let scale = 3.0;
    let h = harness(scale);
    // an unchosen card that is editable shows no pencil
    h.comp.set_solo(card("s", "Solo", false, true, 0.0, 0.0));
    h.advance(0);
    h.move_to(990.0, 630.0);
    let f = h.render();
    let px0 = ((h.comp.get_solo_x() + 160.0) * scale) as u32;
    let py0 = ((h.comp.get_solo_y() + 90.0) * scale) as u32;
    assert_eq!(f.count_where(px0, py0, px0 + 120, py0 + 100, is_accent), 0);
}

#[test]
fn the_preview_paints_the_cards_own_palette() {
    let scale = 3.0;
    let h = harness(scale);
    h.comp.set_solo(card("s", "Solo", false, false, 0.0, 0.0));
    h.advance(0);
    h.move_to(990.0, 630.0);
    let f = h.render();
    let x0 = (h.comp.get_solo_x() * scale) as u32;
    let y0 = (h.comp.get_solo_y() * scale) as u32;
    let (x1, y1) = (x0 + 196 * 3, y0 + 93 * 3);
    let count = |c: [u8; 4]| f.count_where(x0 + 3, y0 + 3, x1 - 3, y1, |p| near(p, c, 30));
    for (name, c) in [
        ("ground", BG),
        ("foreground", FG),
        ("green", GREEN),
        ("yellow", YELLOW),
        ("blue", BLUE),
        ("cyan", CYAN),
        ("red", RED),
    ] {
        assert!(count(c) > 4, "the {name} run is drawn ({})", count(c));
    }
    // the cursor block is 6 by 11
    let cur = count(CURSOR);
    let want = 6 * 11 * 9;
    assert!(
        cur >= want * 9 / 10 && cur <= want * 11 / 10,
        "cursor block {cur} px, want about {want}"
    );
    // the chrome's own ink and accent are nowhere in the preview
    assert_eq!(
        f.count_where(x0 + 3, y0 + 3, x1 - 3, y1, |p| near(p, CHROME_TEXT, 12)),
        0,
        "no chrome text colour in the preview"
    );
    assert_eq!(f.count_where(x0 + 3, y0 + 3, x1 - 3, y1, is_accent), 0);
}

#[test]
fn the_previews_text_sits_on_pts_baselines() {
    // Line 2 ("src docs run.sh") has no descenders, so the bottom of its ink is the baseline. Measured
    // on PT's own 05-settings-appearance frames, from the card's outer top edge: 35.0 css px at 100
    // percent, 34.67 at 150 and 35.67 at 225 (Chromium puts a baseline on a device pixel; 300 percent was not measured, its 35.5 is the middle). Flare's sat
    // 1.3 to 1.8 css px lower (audit V13, 2026-10-21): the same Cascadia Mono, a different centring
    // inside the 15.75 px line.
    for (scale, want, tol) in [
        (1.0_f32, 35.0_f32, 0.2_f32),
        (1.5, 34.67, 0.2),
        // 225 percent sat 0.56 css px high under the old 0.6 tolerance (audit V13-preview, 2026-10-21)
        (2.25, 35.67, 0.2),
        // 300 percent was not measured: its 35.5 is the middle, so the band stays wide
        (3.0, 35.5, 0.6),
    ] {
        let h = harness(scale);
        h.comp.set_solo(card("s", "Solo", false, false, 0.0, 0.0));
        h.advance(0);
        h.move_to(990.0, 630.0);
        let f = h.render();
        let x0 = (h.comp.get_solo_x() * scale) as u32;
        let y0 = (h.comp.get_solo_y() * scale) as u32;
        let ink = |p: [u8; 4]| {
            (0..3)
                .map(|i| (i32::from(p[i]) - i32::from(BG[i])).abs())
                .sum::<i32>()
                > 60
        };
        // the band of line 2 only: from 1 px under line 1's descenders to line 3's ascenders
        let top = y0 + (26.0 * scale) as u32;
        let bottom = y0 + (42.0 * scale) as u32;
        let mut last = None;
        for py in top..bottom {
            if (x0 + 12 * scale as u32..x0 + 90 * scale as u32).any(|px| ink(f.px(px, py))) {
                last = Some(py);
            }
        }
        let last = last.expect("line 2 has ink");
        let baseline = (last + 1) as f32 / scale - h.comp.get_solo_y();
        assert!(
            close(baseline, want, tol),
            "line 2's baseline is {baseline} css px under the card's top at {scale}, PT's is {want}"
        );
    }
}
#[test]
fn a_click_on_a_card_picks_it_and_the_pencil_edits_it() {
    let h = harness(1.0);
    h.comp.set_cards(three_cards());
    h.advance(0);
    let (wx, wy) = (h.comp.get_wall_x(), h.comp.get_wall_y());
    h.click(wx + 30.0, wy + 40.0);
    assert_eq!(h.comp.get_picked_id().as_str(), "a");
    h.click(wx + 208.0 + 30.0, wy + 40.0);
    assert_eq!(h.comp.get_picked_id().as_str(), "b");
    // the pencil of the chosen card
    h.click(wx + 208.0 + 176.0, wy + 126.0 - 16.0);
    assert_eq!(h.comp.get_edited_id().as_str(), "b");
    assert_eq!(
        h.comp.get_picked_id().as_str(),
        "b",
        "the pencil does not pick again"
    );
    // a card's keyboard pick
    h.comp.set_picked_id("".into());
    h.click(wx + 416.0 + 30.0, wy + 40.0);
    assert_eq!(h.comp.get_picked_id().as_str(), "c");
    h.comp.set_picked_id("".into());
    h.key_text(&key(Key::Return));
    assert_eq!(
        h.comp.get_picked_id().as_str(),
        "c",
        "Enter picks the focused card"
    );
}

#[test]
fn a_chosen_cards_pencil_takes_the_keys_when_asked_and_enter_edits() {
    let h = harness(1.0);
    h.comp.set_solo(card("s", "Solo", true, true, 0.0, 0.0));
    h.advance(0);
    h.comp.set_solo_take_pencil(true);
    h.advance(0);
    assert_eq!(
        h.comp.get_pencil_takes(),
        1,
        "the card says it took the keys"
    );
    h.key_text(&key(Key::Return));
    assert_eq!(h.comp.get_edited_id().as_str(), "s");
    assert_eq!(
        h.comp.get_picked_id().as_str(),
        "",
        "Enter on the pencil edits, it does not pick"
    );
}

#[test]
fn the_wall_clips_at_the_given_height_and_grows_over_240_ms() {
    let h = harness(1.0);
    h.comp.set_cards(three_cards());
    h.comp.set_clip_h(268.0);
    h.comp.set_full_h(600.0);
    h.advance(0);
    assert!(close(h.comp.get_clip_now(), 268.0, 0.01));
    assert!(close(h.comp.get_wall_h(), 268.0 + 8.0 + 28.0, 0.01));

    h.comp.set_expanded(true);
    h.advance(0);
    assert!(
        close(h.comp.get_clip_now(), 268.0, 0.5),
        "at 0 ms the clip has not moved: {}",
        h.comp.get_clip_now()
    );
    h.advance(120);
    let mid = h.comp.get_clip_now();
    assert!(
        mid > 268.5 && mid < 599.5,
        "at 120 ms the clip is between: {mid}"
    );
    h.advance(120);
    assert!(
        close(h.comp.get_clip_now(), 600.0, 0.5),
        "at 240 ms the clip is full: {}",
        h.comp.get_clip_now()
    );

    // shrinking is instant
    h.comp.set_expanded(false);
    h.advance(0);
    assert!(
        close(h.comp.get_clip_now(), 268.0, 0.01),
        "shrinking snaps: {}",
        h.comp.get_clip_now()
    );
}

#[test]
fn the_wall_jumps_open_under_reduced_motion() {
    let h = harness(1.0);
    h.comp.set_clip_h(268.0);
    h.comp.set_full_h(600.0);
    h.comp.set_still(true);
    h.advance(0);
    h.comp.set_expanded(true);
    h.advance(0);
    assert!(
        close(h.comp.get_clip_now(), 600.0, 0.01),
        "no animation: {}",
        h.comp.get_clip_now()
    );
}

/// The mean y of the ink in the left and the middle third of the toggle box.
fn chevron_ys(f: &Frame, x0: u32, y0: u32, w: u32, h: u32) -> (f32, f32) {
    let ink = |p: [u8; 4]| p[0] > 70 && p[1] > 70 && p[2] > 70;
    let mean = |xa: u32, xb: u32| -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in y0..y0 + h {
            for x in xa..xb {
                if ink(f.px(x, y)) {
                    sum += y as f32;
                    n += 1.0;
                }
            }
        }
        assert!(n > 0.0, "no chevron ink in {xa}..{xb}");
        sum / n
    };
    (
        mean(x0 + w * 38 / 100, x0 + w * 46 / 100),
        mean(x0 + w * 48 / 100, x0 + w * 52 / 100),
    )
}

#[test]
fn the_toggle_is_40_by_28_and_flips() {
    for scale in SCALES {
        let h = harness(scale);
        h.comp.set_cards(three_cards());
        h.comp.set_clip_h(268.0);
        h.comp.set_full_h(600.0);
        h.advance(0);
        assert!(close(h.comp.get_toggle_w(), 40.0, 0.01), "w at {scale}");
        assert!(close(h.comp.get_toggle_h(), 28.0, 0.01), "h at {scale}");
        let under = h.comp.get_toggle_y() - h.comp.get_wall_y();
        assert!(
            close(under, 268.0 + 8.0, 0.5),
            "8 px under the wall at {scale}: {under}"
        );
        let mid = h.comp.get_toggle_x() + 20.0;
        assert!(
            close(mid, h.comp.get_wall_x() + h.comp.get_wall_w() / 2.0, 0.5),
            "centred at {scale}"
        );
    }
    // the chevron points down, a press turns it up and expands the wall
    let scale = 3.0;
    let h = harness(scale);
    h.comp.set_clip_h(268.0);
    h.comp.set_full_h(400.0);
    h.advance(0);
    let tw = |h: &Harness<WallPreview>| {
        (
            (h.comp.get_toggle_x() * scale) as u32,
            (h.comp.get_toggle_y() * scale) as u32,
        )
    };
    h.move_to(990.0, 630.0);
    let (x0, y0) = tw(&h);
    let f = h.render();
    let (left, mid) = chevron_ys(&f, x0, y0, 120, 84);
    assert!(left < mid, "a down chevron: left {left} above middle {mid}");

    let c = center(h.comp.get_toggle_x(), h.comp.get_toggle_y(), 40.0, 28.0);
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_toggles(), 1);
    assert!(h.comp.get_expanded());
    h.advance(500);
    h.move_to(990.0, 630.0);
    let (x0, y0) = tw(&h);
    let f = h.render();
    let (left, mid) = chevron_ys(&f, x0, y0, 120, 84);
    assert!(left > mid, "an up chevron: left {left} below middle {mid}");
    // and back
    let c = center(h.comp.get_toggle_x(), h.comp.get_toggle_y(), 40.0, 28.0);
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_toggles(), 2);
    assert!(!h.comp.get_expanded());
}

fn open_editor(h: &Harness<WallPreview>) {
    h.comp.set_editor_open(true);
    h.advance(0);
    h.advance(5);
    h.advance(5);
    h.advance(5);
}

#[test]
fn the_editor_has_twenty_wells_in_order_and_a_preview() {
    for scale in [1.0_f32, 1.5, 2.25, 3.0] {
        for wide in [false, true] {
            let h = harness(scale);
            h.comp.set_wells(wells(wide, None));
            open_editor(&h);
            let col_w = h.comp.get_ed_col_w();
            let want_col = 86.0 + 8.0 + if wide { 214.0 } else { 118.0 };
            assert!(close(col_w, want_col, 0.01), "column {col_w} at {scale}");
            let gx = h.comp.get_ed_grid_x();
            let gy = h.comp.get_ed_grid_y();
            for (i, (k, _)) in well_names().into_iter().enumerate() {
                h.comp.set_swatch_key("".into());
                let col = (i % 2) as f32;
                let row = (i / 2) as f32;
                // the swatch is the right-hand 36 px of the field, flush right in the column
                let x = gx + col * (col_w + 24.0) + col_w - 10.0;
                let y = gy + row * 30.0 + 12.0;
                h.click(x, y);
                assert_eq!(
                    h.comp.get_swatch_key().as_str(),
                    k,
                    "well {i} at scale {scale} wide {wide}"
                );
            }
            // the preview card: 196 wide, beside the grid, named Custom and chosen
            assert!(close(h.comp.get_ed_card_w(), 196.0, 0.01));
            assert!(
                close(
                    h.comp.get_ed_card_x(),
                    gx + h.comp.get_ed_grid_w() + 24.0,
                    0.01
                ),
                "the card sits beside the grid"
            );
            assert!(close(
                h.comp.get_ed_card_h(),
                124.0 + 2.0 * hair(scale),
                0.01
            ));
        }
    }
    // pixels: the preview card wears the accent edge
    let scale = 3.0;
    let h = harness(scale);
    open_editor(&h);
    h.move_to(990.0, 630.0);
    let f = h.render();
    let cx = (h.comp.get_ed_card_x() * scale) as u32;
    let cy = (h.comp.get_ed_card_y() * scale) as u32;
    assert!(
        near(f.px(cx + 98 * 3, cy + 1), ACCENT_HI, 8),
        "the preview card is chosen"
    );
    // the box is the modal's: centred
    let bx = h.comp.get_ed_box_x();
    let bw = h.comp.get_ed_box_w();
    assert!(close(bx + bw / 2.0, 500.0, 0.5), "centred box");
    assert!(h.comp.get_ed_box_h() <= 640.0 * 0.85 + 0.5);
}

#[test]
fn the_first_well_takes_the_focus_on_open_and_the_focus_goes_back_on_close() {
    let h = harness(1.0);
    assert!(
        h.comp.get_page_has_focus(),
        "the page holds the focus first"
    );
    open_editor(&h);
    assert!(
        h.comp.get_ed_first_focused(),
        "the first well has the focus"
    );
    assert!(!h.comp.get_page_has_focus());
    h.comp.set_editor_open(false);
    h.advance(0);
    h.advance(0);
    assert_eq!(
        h.comp.get_restores(),
        1,
        "the host is told to restore the focus"
    );
    assert!(h.comp.get_page_has_focus(), "the focus went back");
}

#[test]
fn escape_cancels_the_editor_unless_a_picker_is_open_inside_it() {
    let h = harness(1.0);
    h.comp.set_wells(wells(false, Some(3)));
    open_editor(&h);
    h.key_text(&key(Key::Escape));
    assert_eq!(h.comp.get_cancels(), 0, "a picker is open: Escape is its");
    h.comp.set_wells(wells(false, None));
    h.advance(0);
    h.key_text(&key(Key::Escape));
    assert_eq!(h.comp.get_cancels(), 1);
}

#[test]
fn a_press_on_the_scrim_cancels_and_never_reaches_the_page() {
    let h = harness(1.0);
    open_editor(&h);
    h.click(20.0, 600.0);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_behind_clicks(), 0);
    // a press inside the box does not cancel
    let (bx, by) = (h.comp.get_ed_box_x(), h.comp.get_ed_box_y());
    h.click(bx + 8.0, by + 8.0);
    assert_eq!(h.comp.get_cancels(), 1);
}

#[test]
fn the_buttons_save_and_cancel() {
    let h = harness(1.0);
    open_editor(&h);
    let (sx, sy, sw, sh) = (
        h.comp.get_ed_save_x(),
        h.comp.get_ed_save_y(),
        h.comp.get_ed_save_w(),
        h.comp.get_ed_save_h(),
    );
    assert!(close(sh, 32.0, 0.01));
    let c = center(sx, sy, sw, sh);
    h.click(c.0, c.1);
    assert_eq!(h.comp.get_saves(), 1);
    let cx = h.comp.get_ed_cancel_x() + h.comp.get_ed_cancel_w() / 2.0;
    h.click(cx, c.1);
    assert_eq!(h.comp.get_cancels(), 1);
    // Cancel sits 8 px after Save
    assert!(close(h.comp.get_ed_cancel_x(), sx + sw + 8.0, 0.5));
}

#[test]
fn the_editor_traps_tab() {
    let h = harness(1.0);
    open_editor(&h);
    assert!(h.comp.get_ed_first_focused());
    let mut reached_cancel = false;
    let mut steps = 0;
    for _ in 0..80 {
        h.key_text(&key(Key::Tab));
        h.advance(0);
        steps += 1;
        assert!(
            !h.comp.get_page_has_focus(),
            "Tab left the editor after {steps} presses"
        );
        if h.comp.get_ed_cancel_focused() {
            reached_cancel = true;
            break;
        }
    }
    assert!(reached_cancel, "Tab reaches Cancel in {steps} presses");
    // one more wraps to the first well
    h.key_text(&key(Key::Tab));
    h.advance(0);
    assert!(h.comp.get_ed_first_focused(), "Tab wraps to the first well");
    assert!(!h.comp.get_page_has_focus());
    // and Shift+Tab from the first well wraps to Cancel
    h.key_with(true, false, &key(Key::Tab));
    h.advance(0);
    assert!(h.comp.get_ed_cancel_focused(), "Shift+Tab wraps to Cancel");
    assert!(!h.comp.get_page_has_focus());
    // Backtab as a key of its own does the same from the first well
    h.key_text(&key(Key::Tab));
    h.advance(0);
    assert!(h.comp.get_ed_first_focused());
    h.key_text(&key(Key::Backtab));
    h.advance(0);
    assert!(h.comp.get_ed_cancel_focused());
}

fn open_ask(h: &Harness<WallPreview>) {
    h.comp.set_ask_open(true);
    h.advance(0);
    h.advance(0);
}

#[test]
fn ask_three_answers_cancel_discard_save_and_escape_is_cancel() {
    let click = |which: usize| -> i32 {
        let h = harness(1.0);
        open_ask(&h);
        let (x, y, w, hh) = match which {
            0 => (
                h.comp.get_ask_cancel_x(),
                h.comp.get_ask_cancel_y(),
                h.comp.get_ask_cancel_w(),
                h.comp.get_ask_cancel_h(),
            ),
            1 => (
                h.comp.get_ask_discard_x(),
                h.comp.get_ask_discard_y(),
                h.comp.get_ask_discard_w(),
                h.comp.get_ask_discard_h(),
            ),
            _ => (
                h.comp.get_ask_save_x(),
                h.comp.get_ask_save_y(),
                h.comp.get_ask_save_w(),
                h.comp.get_ask_save_h(),
            ),
        };
        let c = center(x, y, w, hh);
        h.click(c.0, c.1);
        assert_eq!(h.comp.get_answers(), 1, "exactly one answer");
        assert_eq!(h.comp.get_behind_clicks(), 0);
        h.comp.get_answer()
    };
    assert_eq!(click(0), 0, "Cancel");
    assert_eq!(click(1), 1, "Discard");
    assert_eq!(click(2), 2, "Save as Custom");

    // Escape
    let h = harness(1.0);
    open_ask(&h);
    h.key_text(&key(Key::Escape));
    assert_eq!((h.comp.get_answers(), h.comp.get_answer()), (1, 0));
    // a press outside the box
    let h = harness(1.0);
    open_ask(&h);
    h.click(20.0, 600.0);
    assert_eq!((h.comp.get_answers(), h.comp.get_answer()), (1, 0));
    assert_eq!(h.comp.get_behind_clicks(), 0);
    // a press inside the box that misses every button answers nothing
    let h = harness(1.0);
    open_ask(&h);
    h.click(h.comp.get_ask_box_x() + 6.0, h.comp.get_ask_box_y() + 6.0);
    assert_eq!(h.comp.get_answers(), 0);
}

#[test]
fn ask_three_starts_on_save_as_custom_and_tab_cycles_the_three() {
    let h = harness(1.0);
    open_ask(&h);
    assert_eq!(h.comp.get_ask_idx(), 2);
    h.key_text(&key(Key::Tab));
    assert_eq!(h.comp.get_ask_idx(), 0);
    h.key_text(&key(Key::Tab));
    assert_eq!(h.comp.get_ask_idx(), 1);
    h.key_text(&key(Key::Tab));
    assert_eq!(h.comp.get_ask_idx(), 2);
    h.key_with(true, false, &key(Key::Tab));
    assert_eq!(h.comp.get_ask_idx(), 1, "Shift+Tab goes back");
    h.key_text(&key(Key::Return));
    assert_eq!(h.comp.get_answer(), 1, "Enter answers the focused button");
    // closing and opening again starts on Save as Custom
    h.comp.set_ask_open(false);
    h.advance(0);
    open_ask(&h);
    assert_eq!(h.comp.get_ask_idx(), 2);
    h.key_text(&key(Key::Return));
    assert_eq!(h.comp.get_answer(), 2);
}

#[test]
fn ask_three_draws_the_focus_ring_on_save_as_custom() {
    let scale = 3.0;
    let h = harness(scale);
    open_ask(&h);
    h.move_to(990.0, 630.0);
    let f = h.render();
    let sx = (h.comp.get_ask_save_x() * scale) as u32;
    let sy = (h.comp.get_ask_save_y() * scale) as u32;
    let sh = (h.comp.get_ask_save_h() * scale) as u32;
    // the 2 px ring sits just outside the button's left edge
    let left = f.px(sx - 3, sy + sh / 2);
    assert!(
        near(left, ACCENT_HI, 8),
        "ring pixel is accent-hi, got {left:?}"
    );
    // none beside Cancel, whose focus would be a lighter edge
    let cx = (h.comp.get_ask_cancel_x() * scale) as u32;
    let cy = (h.comp.get_ask_cancel_y() * scale) as u32;
    let none = f.px(cx - 3, cy + sh / 2);
    assert!(!is_accent(none), "no ring beside Cancel, got {none:?}");
    // after Tab the ring is gone from Save as Custom
    h.key_text(&key(Key::Tab));
    let f = h.render();
    let left = f.px(sx - 3, sy + sh / 2);
    assert!(!is_accent(left), "the ring moved away, got {left:?}");
}

#[test]
fn ask_three_casts_no_shadow() {
    let scale = 3.0;
    let h = harness(scale);
    open_ask(&h);
    h.move_to(990.0, 630.0);
    let f = h.render();
    // 12 px under the box the scrim is exactly the scrim over the ground
    let bx = (h.comp.get_ask_box_x() * scale) as u32;
    let by = ((h.comp.get_ask_box_y() + h.comp.get_ask_box_h() + 12.0) * scale) as u32;
    let below = f.px(bx + 60, by);
    let far = f.px(20, 600 * 3);
    assert!(
        near(below, far, 1),
        "no shadow under the question: {below:?} vs {far:?}"
    );
}

#[test]
fn the_modals_sit_above_the_page_and_below_a_colour_popover() {
    let scale = 1.0;
    let h = harness(scale);
    h.comp.set_cards(three_cards());
    h.comp.set_popover_shown(true);
    open_editor(&h);
    h.move_to(990.0, 630.0);
    let f = h.render();
    // the scrim dims the page under it
    let dim = f.px(20, 600);
    assert!(
        dim[0] < GROUND[0] - 6 && dim[0] > 2,
        "the page is under the scrim: {dim:?}"
    );
    // the popover stand-in is above the editor box
    let p = f.px(360, 240);
    assert!(near(p, MAGENTA, 1), "the popover is on top: {p:?}");
    // a click on the popover reaches only the popover
    h.click(360.0, 240.0);
    assert_eq!(h.comp.get_popover_clicks(), 1);
    assert_eq!(h.comp.get_behind_clicks(), 0);
    assert_eq!(h.comp.get_cancels(), 0, "the scrim did not see it");
    assert_eq!(h.comp.get_swatch_key().as_str(), "");
    // a click on the page through the scrim cancels and is not the page's
    h.click(20.0, 600.0);
    assert_eq!(h.comp.get_cancels(), 1);
    assert_eq!(h.comp.get_behind_clicks(), 0);
    // the wall's cards are under the scrim too
    h.click(h.comp.get_wall_x() + 30.0, h.comp.get_wall_y() + 40.0);
    assert_eq!(h.comp.get_picked_id().as_str(), "");

    // the question sits above the editor: a press on the editor's Save reaches the question's scrim
    h.comp.set_popover_shown(false);
    open_ask(&h);
    let c = center(
        h.comp.get_ed_save_x(),
        h.comp.get_ed_save_y(),
        h.comp.get_ed_save_w(),
        h.comp.get_ed_save_h(),
    );
    h.click(c.0, c.1);
    assert_eq!(
        h.comp.get_saves(),
        0,
        "the editor's Save is under the question"
    );
    assert_eq!(h.comp.get_answers(), 1);
    assert_eq!(h.comp.get_answer(), 0);
}

#[test]
fn a_well_reports_its_typing_and_its_commit() {
    let h = harness(1.0);
    open_editor(&h);
    // the first well has the focus: type into it
    h.key_text("a");
    assert_eq!(h.comp.get_edit_key().as_str(), "bg");
    assert!(h.comp.get_edit_text().as_str().contains('a'));
    h.key_text(&key(Key::Return));
    assert_eq!(h.comp.get_committed_key().as_str(), "bg");
}
