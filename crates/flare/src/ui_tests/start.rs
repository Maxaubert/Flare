//! component and flow tests for the start screen (Tasks 22 and 31).
//!
//! The screen is measured through the headless harness: rows are found by their rendered
//! colour, text runs by the pixels that change when a label is drawn, and the entrance by the
//! mark's alpha and the block's y over the clock. Every geometry test runs at 1.0 and 2.25.

use std::cell::RefCell;
use std::rc::Rc;

use flare_tabs::keymap::{ChordId, key_label};
use slint::{ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness};
use crate::{Place, StartScreenPreview};

/// The window's size in CSS pixels for these tests.
const W: u32 = 600;
const H: u32 = 400;
/// The footer's GitHub button sits at the bottom left (see start.slint).
const GITHUB_AT: (f32, f32) = (40.0, H as f32 - 18.0);

/// `Theme.ground`, `Theme.accent-solid` and `Theme.divider` over the ground, as rendered.
const GROUND: [u8; 4] = [0x17, 0x17, 0x17, 255];
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];

/// The start screen's nav geometry in CSS pixels, relative to the New row's top: a row is
/// 36 tall, the rows are 2 apart, a divider block is 17 tall, the first place is 57 down and
/// each place 38 further on (start.slint lays the nav out this way).
const ROW_H: f32 = 36.0;
const PLACE_0: f32 = 57.0;
const PLACE_STEP: f32 = 38.0;
const NAV_W: f32 = 340.0;

fn place(path: &str, pinned: bool) -> Place {
    Place {
        path: path.into(),
        name: path.rsplit('\\').next().unwrap_or(path).into(),
        near: "C:\\work".into(),
        label: "".into(),
        pinned,
    }
}

/// The preview with its key labels from the chord table, as AppCtl sets them.
fn preview() -> StartScreenPreview {
    let c = StartScreenPreview::new().unwrap();
    c.set_new_key(key_label(ChordId::NewTab, true).into());
    c.set_settings_key(key_label(ChordId::Settings, true).into());
    c
}

fn screen(scale: f32, places: Vec<Place>) -> Harness<StartScreenPreview> {
    let h = Harness::new(scale, W, H, preview);
    h.comp.set_places(ModelRc::new(VecModel::from(places)));
    h.comp.set_shown(true);
    h.advance(500);
    h
}

/// Top of the place row `i` in CSS pixels, from the New row's top.
fn place_top(new_y: f32, i: usize) -> f32 {
    new_y + PLACE_0 + i as f32 * PLACE_STEP
}

/// Top of the Settings row for `n` places (no places: one divider only).
fn settings_top(new_y: f32, n: usize) -> f32 {
    if n == 0 {
        new_y + 57.0
    } else {
        new_y + 76.0 + n as f32 * PLACE_STEP
    }
}

/// Every device pixel that differs between two frames of the same size, as (x, y).
fn differing(a: &Frame, b: &Frame) -> Vec<(u32, u32)> {
    assert_eq!((a.w, a.h), (b.w, b.h));
    let mut out = Vec::new();
    for y in 0..a.h {
        for x in 0..a.w {
            if a.px(x, y) != b.px(x, y) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Asserts every difference lies in the device-pixel band [y0, y1) (one pixel of slack).
fn assert_only_in_band(diff: &[(u32, u32)], y0: u32, y1: u32, what: &str) {
    assert!(!diff.is_empty(), "{what}: nothing was drawn");
    for &(_, y) in diff {
        assert!(
            y + 1 >= y0 && y < y1 + 1,
            "{what}: a difference at y {y} lies outside the band {y0}..{y1}"
        );
    }
}

#[test]
fn the_rows_are_36_tall_and_340_wide_and_centred() {
    for s in [1.0_f32, 2.25] {
        let h = screen(s, vec![]);
        let new_y = h.comp.get_new_y();
        h.move_to(300.0, new_y + 18.0);
        h.advance(0);
        let f = h.render();
        // The hovered row's own fill, sampled in its left padding (no glyph or text there).
        let fill = f.px(h.phys(134.0), h.phys(new_y + 18.0));
        assert_ne!(fill, GROUND, "scale {s}: the hovered row has no fill");
        // Searched in the nav's band only: the title's anti-aliasing can share the fill's grey.
        let (x0, y0, x1, y1) = f
            .bbox_where(
                h.phys(128.0),
                h.phys(new_y) - 2,
                h.phys(472.0),
                h.phys(new_y + ROW_H) + 2,
                |p| Frame::near(p, fill, 1),
            )
            .expect("the hover fill is drawn");
        let tall = (y1 - y0 + 1) as i64;
        let wide = (x1 - x0 + 1) as i64;
        assert!(
            (tall - h.phys(ROW_H) as i64).abs() <= 1,
            "scale {s}: {tall} device px tall"
        );
        assert!(
            (wide - h.phys(NAV_W) as i64).abs() <= 1,
            "scale {s}: {wide} device px wide"
        );
        let centre2 = (x0 + x1 + 1) as i64; // twice the centre
        assert!(
            (centre2 - f.w as i64).abs() <= 2,
            "scale {s}: the row is off centre by more than 1 px"
        );
    }
}

#[test]
fn new_terminal_places_and_settings_report() {
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    for s in [1.0_f32, 2.25] {
        log.borrow_mut().clear();
        let h = screen(s, vec![place("C:\\a\\dir", false)]);
        let l = log.clone();
        h.comp
            .on_new_terminal(move || l.borrow_mut().push("new".into()));
        let l = log.clone();
        h.comp
            .on_place_picked(move |p| l.borrow_mut().push(format!("place:{p}")));
        let l = log.clone();
        h.comp
            .on_settings(move || l.borrow_mut().push("settings".into()));
        let l = log.clone();
        h.comp
            .on_github(move || l.borrow_mut().push("github".into()));

        let new_y = h.comp.get_new_y();
        h.click(300.0, new_y + 18.0);
        h.click(300.0, place_top(new_y, 0) + 18.0);
        h.click(300.0, settings_top(new_y, 1) + 18.0);
        h.click(GITHUB_AT.0, GITHUB_AT.1);
        assert_eq!(
            log.borrow().as_slice(),
            ["new", "place:C:\\a\\dir", "settings", "github"],
            "scale {s}"
        );
    }
}

#[test]
fn only_pinned_places_say_pinned() {
    // A pinned tag is drawn only in the row that is pinned: compare the frame with the tag
    // on against the frame with no tag at all, and the differences must sit in that row's band.
    let h = screen(
        1.0,
        vec![place("C:\\a\\one", false), place("C:\\a\\two", false)],
    );
    let new_y = h.comp.get_new_y();
    let none = h.render();

    h.comp.set_places(ModelRc::new(VecModel::from(vec![
        place("C:\\a\\one", true),
        place("C:\\a\\two", false),
    ])));
    h.advance(0);
    let first = h.render();
    let band = |i: usize| {
        (
            h.phys(place_top(new_y, i)),
            h.phys(place_top(new_y, i) + ROW_H),
        )
    };
    let (y0, y1) = band(0);
    assert_only_in_band(&differing(&none, &first), y0, y1, "the first place pinned");

    h.comp.set_places(ModelRc::new(VecModel::from(vec![
        place("C:\\a\\one", false),
        place("C:\\a\\two", true),
    ])));
    h.advance(0);
    let second = h.render();
    let (y0, y1) = band(1);
    assert_only_in_band(
        &differing(&none, &second),
        y0,
        y1,
        "the second place pinned",
    );
}

#[test]
fn no_places_means_one_divider_before_settings() {
    let h = screen(1.0, vec![]);
    let new_y = h.comp.get_new_y();
    let f = h.render();
    let x = h.phys(134.0);
    let from = h.phys(new_y + ROW_H);
    let to = h.phys(settings_top(new_y, 0));
    let mut runs = 0;
    let mut inside = false;
    for y in from..to {
        let on = Frame::near(f.px(x, y), DIVIDER, 2);
        if on && !inside {
            runs += 1;
        }
        inside = on;
    }
    assert_eq!(runs, 1, "dividers between the New row and Settings");
}

#[test]
fn the_entrance_plays_once() {
    let h = Harness::new(1.0, W, H, preview);
    h.comp.set_places(ModelRc::new(VecModel::from(vec![])));
    // The first rest is the fade-out from the default `shown: true`: let it finish (each step
    // is drawn, so the animation is evaluated where the test expects it).
    h.comp.set_shown(false);
    h.advance(0);
    let _ = h.render();
    h.advance(500);
    let _ = h.render();
    h.comp.set_shown(true);
    h.advance(0);
    let start = h.render();
    let y_start = h.comp.get_new_y();
    h.advance(420);
    let done = h.render();
    let y_rest = h.comp.get_new_y();

    // The first orange pixel of the finished mark: at the start it is not drawn at all.
    let mut at = None;
    'find: for y in 0..done.h {
        for x in 0..done.w {
            if is_orange(done.px(x, y)) {
                at = Some((x, y));
                break 'find;
            }
        }
    }
    let (x, y) = at.expect("the mark is drawn in the icon's orange");
    // The ground is opaque, so the fade shows as the accent's red channel (ground 23, accent
    // 254) sitting below halfway at the start, and at the accent in the finished frame.
    assert!(
        u32::from(start.px(x, y)[0]) < (23 + 255) / 2,
        "the mark is already half there at the start: {:?}",
        start.px(x, y)
    );
    assert!(is_orange(done.px(x, y)));
    // The block rises 6 px into its place.
    assert!(
        ((y_start - y_rest) - 6.0).abs() < 0.5,
        "start {y_start} rest {y_rest}"
    );
}

/// The artwork's orange flame (`#FF4800` to `#FF4000`) and the golden one over it (`#FFC500` to
/// `#FFD000`), assets/icon/flare-color.svg.
fn is_orange(p: [u8; 4]) -> bool {
    p[3] == 255 && p[0] >= 0xF4 && (0x38..=0x50).contains(&p[1]) && p[2] <= 0x10
}

fn is_gold(p: [u8; 4]) -> bool {
    p[3] == 255 && p[0] >= 0xF4 && (0xBC..=0xD8).contains(&p[1]) && p[2] <= 0x10
}

/// Every pixel the test accepts, as (x, y, green).
fn pixels_of(f: &Frame, is: impl Fn([u8; 4]) -> bool) -> Vec<(u32, u32, u8)> {
    let mut out = Vec::new();
    for y in 0..f.h {
        for x in 0..f.w {
            let p = f.px(x, y);
            if is(p) {
                out.push((x, y, p[1]));
            }
        }
    }
    out
}

fn bbox_of(px: &[(u32, u32, u8)]) -> (u32, u32, u32, u32) {
    let xs = px.iter().map(|p| p.0);
    let ys = px.iter().map(|p| p.1);
    (
        xs.clone().min().unwrap(),
        ys.clone().min().unwrap(),
        xs.max().unwrap(),
        ys.max().unwrap(),
    )
}

/// The text of the quoted `commands:` string that follows `from` in `src`, spaces for commas.
fn commands_after(src: &str, from: usize) -> (String, usize) {
    let at = src[from..].find("commands: \"").unwrap() + from + "commands: \"".len();
    let end = src[at..].find('"').unwrap() + at;
    (src[at..end].replace(',', " "), end)
}

fn svg_path(svg: &str, id: &str) -> String {
    let at = svg.find(&format!("id=\"{id}\"")).unwrap();
    let d = svg[at..].find(" d=\"").unwrap() + at + 4;
    let end = svg[d..].find('"').unwrap() + d;
    svg[d..end].replace(',', " ")
}

#[test]
fn the_color_mark_is_the_artworks_two_paths_and_gradients() {
    // The software renderer these tests use paints a path in its first stop only, so what the
    // GPU shows beyond that (the 135 degree gradients of the SVG's x1=0 y1=0 x2=1 y2=1) is pinned
    // at the source: the two paths are the artwork's own, drawn in one box, in the artwork's stops.
    let slint = include_str!("../../ui/mark.slint");
    let svg = include_str!("../../../../assets/icon/flare-color.svg");
    // The title bar's tinted mark is the same flame outline.
    let mono = &slint[..slint.find("component AppMarkColor").unwrap()];
    assert_eq!(
        commands_after(mono, 0).0,
        svg_path(svg, "flame"),
        "the tinted mark is not the artwork's"
    );
    let colour = &slint[slint.find("component AppMarkColor").unwrap()..];
    let (orange, end) = commands_after(colour, 0);
    let (gold, _) = commands_after(colour, end);
    assert_eq!(
        orange,
        svg_path(svg, "flame"),
        "the orange flame is not the artwork's"
    );
    assert_eq!(
        gold,
        svg_path(svg, "golden-flame"),
        "the golden flame is not the artwork's"
    );
    let first = colour
        .find("fill: @linear-gradient(135deg, IconArt.orange-from 0%, IconArt.orange-to 100%);")
        .unwrap();
    let second = colour
        .find("fill: @linear-gradient(135deg, IconArt.gold-from 0%, IconArt.gold-to 100%);")
        .unwrap();
    assert!(first < second, "the gold goes over the orange");
    let theme = include_str!("../../ui/theme.slint");
    for (id, a, b) in [("orange", "FF4800", "FF4000"), ("gold", "FFC500", "FFD000")] {
        assert!(
            theme.contains(&format!("{id}-from: #{a};"))
                && theme.contains(&format!("{id}-to: #{b};")),
            "{id}: the theme's icon colours are not the artwork's"
        );
        let at = svg.find(&format!("id=\"{id}\"")).unwrap();
        let def = &svg[at..at + svg[at..].find("</linearGradient>").unwrap()];
        assert!(
            def.contains("x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\""),
            "{id}: not corner to corner"
        );
        assert!(
            def.contains(&format!("#{a}")) && def.contains(&format!("#{b}")),
            "{id}: stops"
        );
    }
    // Both paths share one viewbox and one box, so the gold lies exactly over the orange.
    assert_eq!(colour.matches("viewbox-x: 217;").count(), 2);
    assert_eq!(colour.matches("viewbox-width: 818;").count(), 2);
    // The start screen uses it, the old accent flame and the old path are gone.
    let start = include_str!("../../ui/start.slint");
    assert!(start.contains("AppMarkColor {") && !start.contains("AppMark {"));
}

#[test]
fn the_mark_is_the_two_toned_flame_at_84_px() {
    // Owner, 2026-10-22: the flame on the start screen is the icon's own, orange with the golden
    // flame over it (AppMarkColor), not the accent. Its ink is 724 x 816 units of the 818-unit
    // square box (the golden flame is the right-hand part, x 552 to 988 of the artwork's 1254).
    for s in [1.0_f32, 2.25] {
        let h = screen(s, vec![]);
        let f = h.render();
        if let Ok(dir) = std::env::var("FLARE_SAVE_FLAME_PNG") {
            let name = format!("start-screen-{}.png", (s * 100.0) as u32);
            f.save_png(&std::path::Path::new(&dir).join(name)).unwrap();
        }
        let orange = pixels_of(&f, is_orange);
        let gold = pixels_of(&f, is_gold);
        let accent = pixels_of(&f, |p| Frame::near(p, ACCENT, 4));
        assert!(accent.is_empty(), "scale {s}: the old accent flame is back");
        assert!(
            orange.len() > 400,
            "scale {s}: only {} orange pixels",
            orange.len()
        );
        assert!(
            gold.len() > 200,
            "scale {s}: only {} gold pixels",
            gold.len()
        );

        // The ink (orange and gold together): about 74.4 x 83.8 logical px.
        let warm = pixels_of(&f, |p| {
            p[3] == 255 && p[0] > 0x80 && p[0] >= p[1] && p[2] < 0x30
        });
        let (ox0, oy0, ox1, oy1) = bbox_of(&warm);
        let (w, ht) = ((ox1 - ox0 + 1) as f32 / s, (oy1 - oy0 + 1) as f32 / s);
        assert!(
            (w - 74.4).abs() <= 3.0,
            "scale {s}: ink {w} wide ({ox0}..{ox1})"
        );
        assert!((ht - 83.8).abs() <= 3.0, "scale {s}: ink {ht} tall");

        // The golden flame sits exactly over the orange one: inside its ink, its left edge
        // (552 - 265) / 724 of the way across, its right edge level with the orange's.
        let (gx0, gy0, gx1, gy1) = bbox_of(&gold);
        let tol = (2.0 * s).ceil() as u32 + 1;
        assert!(
            gx0 >= ox0 && gx1 <= ox1 + tol && gy0 >= oy0 && gy1 <= oy1,
            "scale {s}: gold leaves the orange"
        );
        assert!(
            gx1.abs_diff(ox1) <= tol,
            "scale {s}: the right edges differ"
        );
        let want = ox0 as f32 + (552.2 - 264.56) / 723.85 * (ox1 - ox0 + 1) as f32;
        assert!(
            (gx0 as f32 - want).abs() <= 3.0 * s,
            "scale {s}: gold starts at {gx0}, want {want}"
        );
    }
}

#[test]
fn a_long_place_name_elides_and_never_pushes_pinned_out() {
    let long = format!("C:\\{}", "n".repeat(90));
    let h = screen(1.0, vec![place(&long, false)]);
    let new_y = h.comp.get_new_y();
    let none = h.render();
    h.comp
        .set_places(ModelRc::new(VecModel::from(vec![place(&long, true)])));
    h.advance(0);
    let pinned = h.render();
    let diff = differing(&none, &pinned);
    let (y0, y1) = (
        h.phys(place_top(new_y, 0)),
        h.phys(place_top(new_y, 0) + ROW_H),
    );
    assert_only_in_band(&diff, y0, y1, "the pinned tag of a long name");
    // The tag stays inside the row's right edge.
    let right = h.phys(130.0 + NAV_W);
    assert!(
        diff.iter().all(|&(x, _)| x < right),
        "the pinned tag is pushed past the row"
    );
}

#[test]
fn the_start_screen_has_no_greeting_and_no_explanation() {
    // The words the screen draws as literals are the wordmark, the pinned tag and the footer link.
    // Place names, key labels and the version come from the app, never from the screen's own text.
    let src = include_str!("../../ui/start.slint");
    let mut drawn: Vec<&str> = src
        .lines()
        // A property's default ("Version 0.1.0" in the preview) is the app's text, set at run time.
        .filter(|l| !l.contains("property"))
        .filter_map(|l| l.split_once("text: \"").map(|(_, rest)| rest))
        .filter_map(|rest| rest.split_once('"').map(|(words, _)| words))
        .collect();
    drawn.sort_unstable();
    assert_eq!(
        drawn,
        ["Flare", "GitHub", "Pinned"],
        "the screen draws no greeting or explanation of its own"
    );
}

/// The ink in a device-pixel box, by column, grouped into runs that `gap` empty columns
/// split: each run's (x0, y0, x1, y1).
fn ink_runs(
    f: &Frame,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
    gap: u32,
) -> Vec<(u32, u32, u32, u32)> {
    let ink = |p: [u8; 4]| !Frame::near(p, GROUND, 12);
    let mut runs: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut last: Option<u32> = None;
    for x in x0..x1 {
        let Some((_, b, _, d)) = f.bbox_where(x, y0, x + 1, y1, ink) else {
            continue;
        };
        match (last, runs.last_mut()) {
            (Some(l), Some(r)) if x - l <= gap => {
                r.1 = r.1.min(b);
                r.2 = x;
                r.3 = r.3.max(d);
            }
            _ => runs.push((x, b, x, d)),
        }
        last = Some(x);
    }
    runs
}

#[test]
fn every_row_starts_at_the_left_with_its_glyph_centred_on_the_row() {
    // Visual review (Task 33): each row was centred on its own and its glyph sat at the
    // row's top. PT lays every row out from the left: the glyph 10 px in, centred on the
    // row, the name 12 px after it, so the glyphs make one column.
    for s in [1.0_f32, 1.5, 2.25] {
        let h = screen(
            s,
            vec![
                place("C:\\a\\api", true),
                place("C:\\a\\a-much-longer-name", false),
            ],
        );
        let new_y = h.comp.get_new_y();
        let f = h.render();
        let left = (W as f32 - NAV_W) / 2.0;
        let tops = [
            new_y,
            place_top(new_y, 0),
            place_top(new_y, 1),
            settings_top(new_y, 2),
        ];
        for (i, top) in tops.iter().enumerate() {
            let boxed = (
                h.phys(left),
                h.phys(*top),
                h.phys(left + NAV_W),
                h.phys(top + ROW_H),
            );
            let runs = ink_runs(&f, boxed, h.phys(5.0));
            let (gx0, gy0, gx1, gy1) = *runs.first().expect("the row draws a glyph");
            assert!(
                gx0 + 1 >= h.phys(left + 10.0) && gx1 <= h.phys(left + 26.0) + 1,
                "scale {s} row {i}: the glyph spans {gx0}..{gx1}, not inside {}..{}",
                h.phys(left + 10.0),
                h.phys(left + 26.0)
            );
            let mid2 = i64::from(gy0 + gy1 + 1);
            let row2 = i64::from(h.phys(*top) + h.phys(top + ROW_H));
            assert!(
                (mid2 - row2).abs() <= 2 * i64::from(h.phys(1.5)),
                "scale {s} row {i}: the glyph's centre is off the row's by {} device px",
                (mid2 - row2) / 2
            );
            let (nx0, ..) = runs.get(1).copied().expect("the row draws a name");
            let want = i64::from(h.phys(left + 38.0));
            assert!(
                (i64::from(nx0) - want).abs() <= i64::from(h.phys(2.0)),
                "scale {s} row {i}: the name starts at {nx0}, want about {want}"
            );
            // The key (New, Settings) and the Pinned tag end at the row's right padding.
            if i != 2 {
                let (.., rx1, _) = *runs.last().expect("the row draws its end");
                let want = i64::from(h.phys(left + NAV_W - 10.0));
                assert!(
                    (i64::from(rx1) - want).abs() <= i64::from(h.phys(2.0)),
                    "scale {s} row {i}: the row's end text ends at {rx1}, want about {want}"
                );
            }
        }
    }
}
