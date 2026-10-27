//! Mark component tests at the eight scales (Tasks 24 and 25).

use flare_agent::mark_motion::run_bar_x;
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{
    MarkMotion, MarkPreview, MenusPreview, OverflowRow, StripGeom, TabRow, TabSidebarPreview,
    TabStripPreview, Theme,
};

const GROUND: [u8; 4] = [23, 23, 23, 255];
const INK: [u8; 4] = [0xde, 0xda, 0xcf, 255];
const WORKING: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const QUESTION: [u8; 4] = [0x3b, 0x82, 0xf6, 255];
const FAILED: [u8; 4] = [0xff, 0x61, 0x5a, 255];
const FINISHED: [u8; 4] = [0x22, 0xc5, 0x5e, 255];
const CLAWD: [u8; 4] = [0xd7, 0x77, 0x57, 255];
const RAINBOW: [[u8; 3]; 7] = [
    [0x12, 0xce, 0xe5],
    [0x21, 0x79, 0xfa],
    [0x94, 0x5a, 0xf5],
    [0xe8, 0x4f, 0xd2],
    [0xfb, 0x7f, 0x6c],
    [0xf0, 0xa9, 0x34],
    [0xe8, 0xd0, 0x21],
];

fn col(c: [u8; 4]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

fn harness(scale: f32) -> Harness<MarkPreview> {
    let h = Harness::new(scale, 400, 36, || MarkPreview::new().unwrap());
    h.comp.set_ink(col(INK));
    h.comp.set_ground(col(GROUND));
    set_motion(&h, MotionSet::default());
    h
}

#[derive(Clone, Copy)]
struct MotionSet {
    still: bool,
    spin: f32,
    breathe: f32,
    run: f32,
    flow: f32,
}

impl Default for MotionSet {
    fn default() -> Self {
        Self {
            still: false,
            spin: 0.0,
            breathe: 1.0,
            run: 0.5,
            flow: 0.0,
        }
    }
}

fn set_motion(h: &Harness<MarkPreview>, m: MotionSet) {
    let g = h.comp.global::<MarkMotion>();
    g.set_still(m.still);
    g.set_spin(m.spin);
    g.set_breathe(m.breathe);
    g.set_run(m.run);
    g.set_flow(m.flow);
}

fn set_rainbow(h: &Harness<MarkPreview>) {
    let stops: Vec<Color> = RAINBOW
        .iter()
        .map(|c| Color::from_rgb_u8(c[0], c[1], c[2]))
        .collect();
    h.comp
        .global::<MarkMotion>()
        .set_rainbow(ModelRc::new(VecModel::from(stops)));
}

/// A bar of `kind` (2 line, 3 run) in `state` coloured `colour`.
fn bar(scale: f32, kind: i32, state: i32, colour: [u8; 4]) -> Harness<MarkPreview> {
    let h = harness(scale);
    h.comp.set_mark_kind(kind);
    h.comp.set_mark_state(state);
    h.comp.set_mark_color(col(colour));
    h
}

/// A ring in `state` in the 16 px end box.
fn ring(scale: f32, state: i32) -> Harness<MarkPreview> {
    let h = harness(scale);
    h.comp.set_mark_kind(1);
    h.comp.set_mark_state(state);
    h
}

fn differs(p: [u8; 4]) -> bool {
    p.iter()
        .zip(GROUND.iter())
        .take(3)
        .any(|(a, b)| a.abs_diff(*b) > 30)
}

/// Any pixel that is not exactly the ground, however faint (the tangent pixels of a circle carry almost no coverage).
fn faint(p: [u8; 4]) -> bool {
    p.iter().zip(GROUND.iter()).take(3).any(|(a, b)| a != b)
}

fn bar_rows(scale: f32) -> u32 {
    (3.0 * scale).round() as u32
}

/// The y of the middle row of the bar band.
fn band_mid(f: &Frame, scale: f32) -> u32 {
    f.h - bar_rows(scale).div_ceil(2)
}

/// The box (device px) of the 16 css px end box: x0, y0, x1, y1.
fn end_box(s: f32) -> (u32, u32, u32, u32) {
    (
        (100.0 * s) as u32 - 2,
        (10.0 * s) as u32 - 2,
        (116.0 * s) as u32 + 2,
        (26.0 * s) as u32 + 2,
    )
}

fn centre_of(b: (u32, u32, u32, u32)) -> (f32, f32) {
    ((b.0 + b.2 + 1) as f32 / 2.0, (b.1 + b.3 + 1) as f32 / 2.0)
}

fn track() -> [u8; 4] {
    let mix = |g: u8, i: u8| (f32::from(g) + 0.24 * (f32::from(i) - f32::from(g))).round() as u8;
    [
        mix(GROUND[0], INK[0]),
        mix(GROUND[1], INK[1]),
        mix(GROUND[2], INK[2]),
        255,
    ]
}

#[test]
fn the_line_is_three_css_pixels_snapped_at_every_scale() {
    let want = [3u32, 4, 5, 5, 6, 7, 8, 9];
    for (s, want) in SCALES.into_iter().zip(want) {
        let h = bar(s, 2, 3, FINISHED);
        let f = h.render();
        let x = (10.0 * s) as u32;
        let is_line = |p: [u8; 4]| Frame::near(p, FINISHED, 2);
        let rows = f.count_where(x, 0, x + 1, f.h, is_line);
        assert_eq!(rows as u32, want, "scale {s}");
        assert_eq!(bar_rows(s), want, "scale {s}");
        assert!(
            Frame::near(f.px(x, f.h - want - 1), GROUND, 2),
            "the row above the line is the ground at {s}"
        );
        assert!(
            is_line(f.px(x, f.h - 1)),
            "the line sits on the foot at {s}"
        );
    }
}

#[test]
fn the_line_spans_the_tab_edge_to_edge_in_its_exact_colour() {
    for s in [1.0f32, 1.5, 2.25] {
        let h = bar(s, 2, 3, FINISHED);
        let f = h.render();
        let y = band_mid(&f, s);
        let run = f.run_len_h(y, 0, |p| p == FINISHED);
        assert_eq!(run, (150.0 * s).round() as u32, "scale {s}");
    }
}

#[test]
fn failed_is_still_and_question_breathes() {
    let s = 2.0;
    // Failed does not move with the breath.
    let h = bar(s, 2, 4, FAILED);
    let f = h.render();
    let y = band_mid(&f, s);
    let before = f.px(40, y);
    set_motion(
        &h,
        MotionSet {
            breathe: 0.0,
            ..MotionSet::default()
        },
    );
    assert_eq!(h.render().px(40, y), before);
    assert!(Frame::near(before, FAILED, 1));

    let h = bar(s, 2, 2, QUESTION);
    let at = |b: f32| {
        set_motion(
            &h,
            MotionSet {
                breathe: b,
                ..MotionSet::default()
            },
        );
        let f = h.render();
        f.px(40, band_mid(&f, s))
    };
    assert!(Frame::near(at(0.0), GROUND, 2));
    let half = [
        ((u32::from(GROUND[0]) + u32::from(QUESTION[0])) / 2) as u8,
        ((u32::from(GROUND[1]) + u32::from(QUESTION[1])) / 2) as u8,
        ((u32::from(GROUND[2]) + u32::from(QUESTION[2])) / 2) as u8,
        255,
    ];
    assert!(Frame::near(at(0.5), half, 2), "half {:?}", at(0.5));
    assert!(Frame::near(at(1.0), QUESTION, 2));
}

#[test]
fn the_run_bar_is_42_percent_wide_round_ended_and_travels() {
    let s = 2.0;
    let h = bar(s, 3, 1, WORKING);
    set_motion(
        &h,
        MotionSet {
            run: 0.5,
            ..MotionSet::default()
        },
    );
    let f = h.render();
    let mid = band_mid(&f, s);
    let top = f.h - bar_rows(s);
    let is_c = |p: [u8; 4]| Frame::near(p, WORKING, 8);
    let (x0, _, x1, _) = f
        .bbox_where(0, mid, f.w, mid + 1, is_c)
        .expect("the bar shows");
    let width = x1 - x0 + 1;
    let want_w = (0.42f32 * 300.0).round() as i64;
    assert!((i64::from(width) - want_w).abs() <= 2, "width {width}");
    let want_x = run_bar_x(0.5, 150.0) * 2.0;
    assert!((x0 as f32 - want_x).abs() <= 2.0, "x {x0} vs {want_x}");
    assert!(
        !is_c(f.px(x0, top)),
        "the left end of the top row is rounded"
    );
    assert!(
        !is_c(f.px(x1, top)),
        "the right end of the top row is rounded"
    );

    for run in [0.0f32, 1.0] {
        set_motion(
            &h,
            MotionSet {
                run,
                ..MotionSet::default()
            },
        );
        let f = h.render();
        let n = f.count_where(0, f.h - bar_rows(s), f.w, f.h, differs);
        // Only the band is inspected: the ground elsewhere is the same colour anyway.
        assert_eq!(n, 0, "no colour in the band at run {run}");
    }
}

#[test]
fn a_still_run_is_a_full_line() {
    for s in [1.0f32, 1.5, 2.25] {
        let h = bar(s, 3, 1, WORKING);
        set_motion(
            &h,
            MotionSet {
                still: true,
                run: 0.3,
                ..MotionSet::default()
            },
        );
        let f = h.render();
        let y = band_mid(&f, s);
        let run = f.run_len_h(y, 0, |p| Frame::near(p, WORKING, 1));
        assert_eq!(run, (150.0 * s).round() as u32, "scale {s}");
    }
}

#[test]
fn the_rainbow_line_shows_the_seven_colours_and_flows_one_tab_per_lap() {
    let h = bar(1.0, 2, 3, FINISHED);
    h.comp.set_mark_rainbow(true);
    set_rainbow(&h);
    let f = h.render();
    let y = band_mid(&f, 1.0);
    for (x, stop) in [0u32, 16, 33, 49, 65, 81, 98].into_iter().zip(RAINBOW) {
        let p = f.px(x, y);
        assert!(
            Frame::near(p, [stop[0], stop[1], stop[2], 255], 12),
            "x {x}: {p:?} vs {stop:?}"
        );
    }
    set_motion(
        &h,
        MotionSet {
            flow: 57.0,
            ..MotionSet::default()
        },
    );
    let g = h.render();
    for x in (57u32..150).step_by(5) {
        let moved = g.px(x, y);
        let was = f.px(x - 57, y);
        assert!(Frame::near(moved, was, 8), "x {x}: {moved:?} vs {was:?}");
    }
    set_motion(
        &h,
        MotionSet {
            flow: 114.0,
            ..MotionSet::default()
        },
    );
    let lap = h.render();
    for x in (0u32..150).step_by(7) {
        assert!(Frame::near(lap.px(x, y), f.px(x, y), 3), "lap at {x}");
    }
}

#[test]
fn the_rainbow_off_is_the_finished_colour_still() {
    let h = bar(1.0, 2, 3, FINISHED);
    h.comp.set_mark_rainbow(false);
    set_rainbow(&h);
    for flow in [0.0f32, 40.0, 100.0] {
        set_motion(
            &h,
            MotionSet {
                flow,
                ..MotionSet::default()
            },
        );
        let f = h.render();
        let y = band_mid(&f, 1.0);
        assert_eq!(f.run_len_h(y, 0, |p| p == FINISHED), 150, "flow {flow}");
    }
}

#[test]
fn the_spinner_is_a_twelve_pixel_circle_centred_in_its_slot() {
    for s in SCALES {
        let h = ring(s, 1);
        let f = h.render();
        let b = end_box(s);
        let bb = f
            .bbox_where(b.0, b.1, b.2, b.3, faint)
            .expect("the spinner shows");
        let size = (12.0 * s) as i64;
        assert!(
            ((bb.2 - bb.0 + 1) as i64 - size).abs() <= 1,
            "wide at {s}: {bb:?}"
        );
        assert!(
            ((bb.3 - bb.1 + 1) as i64 - size).abs() <= 1,
            "high at {s}: {bb:?}"
        );
        let (cx, cy) = centre_of(bb);
        assert!((cx - 108.0 * s).abs() <= 1.0, "cx {cx} at {s}");
        assert!((cy - 18.0 * s).abs() <= 1.0, "cy {cy} at {s}");
    }
}

/// The pixel on the spinner's centreline at (dx, dy) in units of the radius (5 css px) from the
/// ring's centre, measured from the ring's own pixels: a ring sits at fractional device pixels at
/// some scales, so its centre is found, not computed.
fn on_ring(f: &Frame, s: f32, dx: f32, dy: f32) -> [u8; 4] {
    let b = end_box(s);
    let bb = f
        .bbox_where(b.0, b.1, b.2, b.3, faint)
        .expect("the ring shows");
    let (cx, cy) = centre_of(bb);
    f.px(
        (cx + dx * 5.0 * s - 0.5).round() as u32,
        (cy + dy * 5.0 * s - 0.5).round() as u32,
    )
}

#[test]
fn the_spinner_arc_is_the_top_quarter_and_turns() {
    let s = 2.25;
    let h = ring(s, 1);
    let f = h.render();
    assert!(Frame::near(on_ring(&f, s, 0.0, -1.0), INK, 8), "top solid");
    assert!(
        Frame::near(on_ring(&f, s, 0.0, 1.0), track(), 6),
        "bottom track"
    );
    assert!(
        Frame::near(on_ring(&f, s, 1.0, 0.0), track(), 6),
        "right track"
    );
    assert!(
        Frame::near(on_ring(&f, s, -1.0, 0.0), track(), 6),
        "left track"
    );
    set_motion(
        &h,
        MotionSet {
            spin: 90.0,
            ..MotionSet::default()
        },
    );
    let f = h.render();
    assert!(Frame::near(on_ring(&f, s, 1.0, 0.0), INK, 8), "right solid");
    assert!(
        Frame::near(on_ring(&f, s, 0.0, -1.0), track(), 6),
        "top track"
    );
}

#[test]
fn a_still_spinner_has_two_coloured_sides() {
    let s = 2.25;
    let h = ring(s, 1);
    set_motion(
        &h,
        MotionSet {
            still: true,
            spin: 200.0,
            ..MotionSet::default()
        },
    );
    let f = h.render();
    assert!(Frame::near(on_ring(&f, s, 0.0, -1.0), INK, 8), "top solid");
    assert!(Frame::near(on_ring(&f, s, 1.0, 0.0), INK, 8), "right solid");
    assert!(
        Frame::near(on_ring(&f, s, 0.0, 1.0), track(), 6),
        "bottom track"
    );
    assert!(
        Frame::near(on_ring(&f, s, -1.0, 0.0), track(), 6),
        "left track"
    );
}

#[test]
fn the_question_dot_is_seven_pixels_and_breathes() {
    for s in SCALES {
        let h = ring(s, 2);
        let f = h.render();
        let b = end_box(s);
        let bb = f
            .bbox_where(b.0, b.1, b.2, b.3, differs)
            .expect("the dot shows");
        let size = (7.0 * s) as i64;
        assert!(
            ((bb.2 - bb.0 + 1) as i64 - size).abs() <= 1,
            "wide at {s}: {bb:?}"
        );
        assert!(
            ((bb.3 - bb.1 + 1) as i64 - size).abs() <= 1,
            "high at {s}: {bb:?}"
        );
        let (cx, cy) = centre_of(bb);
        assert!((cx - 108.0 * s).abs() <= 1.0, "cx {cx} at {s}");
        assert!((cy - 18.0 * s).abs() <= 1.0, "cy {cy} at {s}");
        set_motion(
            &h,
            MotionSet {
                breathe: 0.0,
                ..MotionSet::default()
            },
        );
        let f = h.render();
        assert!(
            f.bbox_where(b.0, b.1, b.2, b.3, differs).is_none(),
            "the dot is gone at breathe 0 ({s})"
        );
    }
}

#[test]
fn the_check_and_the_cross_are_mono_in_the_names_ink() {
    for (state, palette, limit) in [(3, FINISHED, 12.0f32), (4, FAILED, 10.0)] {
        for s in [1.0f32, 1.5, 2.25] {
            let h = ring(s, state);
            h.comp.set_mark_color(col(palette));
            let f = h.render();
            let b = end_box(s);
            let bb = f
                .bbox_where(b.0, b.1, b.2, b.3, differs)
                .expect("the glyph shows");
            assert!(
                ((bb.2 - bb.0 + 1) as f32) <= limit * s + 1.0,
                "wide at {s}: {bb:?}"
            );
            assert!(
                ((bb.3 - bb.1 + 1) as f32) <= limit * s + 1.0,
                "high at {s}: {bb:?}"
            );
            let solid = f.count_where(b.0, b.1, b.2, b.3, |p| Frame::near(p, INK, 6));
            assert!(solid > 0, "state {state} has solid ink pixels at {s}");
            let palette_px = f.count_where(b.0, b.1, b.2, b.3, |p| Frame::near(p, palette, 20));
            assert_eq!(
                palette_px, 0,
                "state {state} never wears the palette at {s}"
            );
        }
    }
}

#[test]
fn every_mark_is_centred_in_its_slot_at_every_scale() {
    for state in 1..=4 {
        for s in SCALES {
            let h = ring(s, state);
            let f = h.render();
            let b = end_box(s);
            let bb = f
                .bbox_where(b.0, b.1, b.2, b.3, differs)
                .expect("the mark shows");
            let (cx, cy) = centre_of(bb);
            assert!(
                (cx - 108.0 * s).abs() <= 1.0,
                "state {state} cx {cx} at {s}"
            );
            assert!((cy - 18.0 * s).abs() <= 1.0, "state {state} cy {cy} at {s}");
        }
    }
}

#[test]
fn the_icon_tab_corner_disc_holds_the_ring() {
    for s in [1.0f32, 1.5, 2.25, 3.0] {
        let h = ring(s, 3);
        h.comp
            .global::<Theme>()
            .set_tabs(Color::from_rgb_u8(0x30, 0x30, 0x30));
        let f = h.render();
        // The disc's centre and the check inside it.
        let (dx0, dy0, dx1, dy1) = (
            (300.0 * s) as u32,
            (10.0 * s) as u32,
            (315.0 * s).ceil() as u32,
            (25.0 * s).ceil() as u32,
        );
        let disc = f.px(dx0 + 2, (dy0 + dy1) / 2);
        assert!(
            Frame::near(disc, [0x30, 0x30, 0x30, 255], 2),
            "the disc is Theme.tabs at {s}: {disc:?}"
        );
        let bb = f
            .bbox_where(dx0, dy0, dx1, dy1, |p| {
                (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 120
            })
            .expect("the check shows in the disc");
        let (cx, cy) = centre_of(bb);
        assert!((cx - 307.5 * s).abs() <= 1.5, "cx {cx} at {s}");
        assert!((cy - 17.5 * s).abs() <= 1.5, "cy {cy} at {s}");
    }
}

#[test]
fn agent_icons_are_eighteen_pixels() {
    for s in [1.0f32, 1.5, 2.25] {
        let region = (
            (330.0 * s) as u32,
            0u32,
            (380.0 * s) as u32,
            (36.0 * s) as u32,
        );
        for kind in 0..5 {
            let h = harness(s);
            h.comp.set_icon_kind(kind);
            let f = h.render();
            let bb = f
                .bbox_where(region.0, region.1, region.2, region.3, differs)
                .unwrap_or_else(|| panic!("icon {kind} shows at {s}"));
            let (bx0, bx1) = (340.0 * s - 1.0, 358.0 * s + 1.0);
            let (by0, by1) = (9.0 * s - 1.0, 27.0 * s + 1.0);
            assert!(
                bb.0 as f32 >= bx0 && (bb.2 + 1) as f32 <= bx1,
                "icon {kind} fits across at {s}: {bb:?}"
            );
            assert!(
                bb.1 as f32 >= by0 && (bb.3 + 1) as f32 <= by1,
                "icon {kind} fits down at {s}: {bb:?}"
            );
            let clawd_px = f.count_where(region.0, region.1, region.2, region.3, |p| {
                Frame::near(p, CLAWD, 4)
            });
            let ink_px = f.count_where(region.0, region.1, region.2, region.3, |p| {
                Frame::near(p, INK, 10)
            });
            if kind == 1 {
                assert!(clawd_px > 0, "Clawd is orange at {s}");
                assert_eq!(ink_px, 0, "Clawd wears no ink at {s}");
                let cb = f
                    .bbox_where(region.0, region.1, region.2, region.3, |p| {
                        Frame::near(p, CLAWD, 40)
                    })
                    .unwrap();
                let (w, hgt) = (cb.2 - cb.0 + 1, cb.3 - cb.1 + 1);
                assert!(w > hgt, "Clawd is wider than tall at {s}: {w} x {hgt}");
            } else if kind == 3 {
                // The agy arch: its own colours, never the ink. A cell is 18/11 px, so at 100 % two
                // one-cell columns fall across pixel edges and blend; from 150 % every colour shows pure.
                assert_eq!(ink_px, 0, "the agy arch wears no ink at {s}");
                let shown = flare_agent::icon::AGY_COLOURS
                    .iter()
                    .filter(|hex| {
                        let c = u32::from_str_radix(&hex[1..], 16).unwrap();
                        let want = [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255];
                        f.count_where(region.0, region.1, region.2, region.3, |p| {
                            Frame::near(p, want, 6)
                        }) > 0
                    })
                    .count();
                let want = if s < 1.5 { 8 } else { 10 };
                assert!(
                    shown >= want,
                    "the agy arch shows {shown} of its colours at {s}"
                );
            } else {
                assert_eq!(clawd_px, 0, "icon {kind} is never Clawd's orange at {s}");
                assert!(ink_px > 0, "icon {kind} is the ink at {s}");
            }
        }
    }
}

/// The icon's box at (340, 9), and what of it is near `want`: its bounding box and pixel count.
fn icon_ink(f: &Frame, s: f32, want: [u8; 4], tol: u8) -> (Option<(u32, u32, u32, u32)>, usize) {
    let r = (
        (330.0 * s) as u32,
        0u32,
        (380.0 * s) as u32,
        (36.0 * s) as u32,
    );
    let pick = |p: [u8; 4]| Frame::near(p, want, tol);
    (
        f.bbox_where(r.0, r.1, r.2, r.3, pick),
        f.count_where(r.0, r.1, r.2, r.3, pick),
    )
}

/// Owner decision 2026-10-26: the icon moves by the frame the mark clock pushes, only with a motion given, and a
/// still clock draws it as it stands. The hop lifts and squashes the box, H1 shuts Clawd's eyes, O6 and O4 fade, O4
/// shrinks, and A1 paints each path of agy's arch the colour it is given. (The software renderer leaves a turn out.)
#[test]
fn agent_icons_move_by_the_pushed_frame_and_stand_still_when_stilled() {
    use flare_agent::icon_motion::{AGY_RGB, IconMotion as M};
    let s = 2.0f32;
    let draw = |kind: i32, motion: M, set: &dyn Fn(&MarkMotion)| {
        let h = harness(s);
        h.comp.set_icon_kind(kind);
        h.comp.set_icon_motion(motion.code());
        set(&h.comp.global::<MarkMotion>());
        h.render()
    };
    let hop = |g: &MarkMotion| {
        g.set_hop_y(-0.22);
        g.set_hop_sx(0.96);
        g.set_hop_sy(1.06);
    };
    let (rest, rest_n) = icon_ink(&draw(1, M::None, &|_| {}), s, CLAWD, 40);
    let rest = rest.unwrap();
    // The hop: up by a fifth of the icon (18 css px * 0.22 = 4 at 200 %: 8 device px), taller and narrower.
    let (up, _) = icon_ink(&draw(1, M::HopBlink, &hop), s, CLAWD, 40);
    let up = up.unwrap();
    assert!(rest.1 - up.1 >= 6, "{rest:?} -> {up:?}");
    assert!(
        up.2 - up.0 < rest.2 - rest.0,
        "narrower: {rest:?} -> {up:?}"
    );
    assert!(up.3 - up.1 > rest.3 - rest.1, "taller: {rest:?} -> {up:?}");
    // The landing: squashed, wider and shorter (an even shrink would be narrower too).
    let land = |g: &MarkMotion| {
        g.set_hop_sx(1.08);
        g.set_hop_sy(0.88);
    };
    let (down, _) = icon_ink(&draw(1, M::HopBlink, &land), s, CLAWD, 40);
    let down = down.unwrap();
    assert!(
        down.2 - down.0 > rest.2 - rest.0,
        "wider: {rest:?} -> {down:?}"
    );
    assert!(
        down.3 - down.1 < rest.3 - rest.1,
        "shorter: {rest:?} -> {down:?}"
    );
    // The same frame with no motion given, or with the clock stood, is the icon at rest.
    assert_eq!(
        icon_ink(&draw(1, M::None, &hop), s, CLAWD, 40).0,
        Some(rest)
    );
    let stood = |g: &MarkMotion| {
        hop(g);
        g.set_still(true);
        g.set_blink(true);
    };
    assert_eq!(
        icon_ink(&draw(1, M::HopBlink, &stood), s, CLAWD, 40),
        (Some(rest), rest_n)
    );
    // The blink fills both eyes in Clawd's colour, only with H1.
    let shut = |g: &MarkMotion| g.set_blink(true);
    let (_, shut_n) = icon_ink(&draw(1, M::HopBlink, &shut), s, CLAWD, 40);
    let eye = (2.0 * 18.0 / 17.0 * s) as usize; // a cell is 1.02 x 2.04 of 17 units
    assert!(shut_n >= rest_n + 2 * eye, "{rest_n} -> {shut_n}");
    assert_eq!(icon_ink(&draw(1, M::Hop, &shut), s, CLAWD, 40).1, rest_n);
    // O6's fade: the ink is drawn at 0.55, so no pixel is the full ink any more.
    let faded = |g: &MarkMotion| g.set_fade(0.55);
    assert!(icon_ink(&draw(2, M::None, &faded), s, INK, 10).1 > 0);
    assert_eq!(icon_ink(&draw(2, M::SpinFade, &faded), s, INK, 10).1, 0);
    // O4: smaller and fainter.
    let small = |g: &MarkMotion| {
        g.set_grow(0.82);
        g.set_grow_alpha(0.7);
    };
    let (big, _) = icon_ink(&draw(2, M::None, &small), s, INK, 120);
    let (little, _) = icon_ink(&draw(2, M::Breathe, &small), s, INK, 120);
    let (big, little) = (big.unwrap(), little.unwrap());
    assert!(
        little.2 - little.0 + 4 <= big.2 - big.0,
        "{big:?} -> {little:?}"
    );
    // A1: every path wears the colour pushed for it; still, the arch is its own colours again.
    let one = AGY_RGB[5];
    let red = |g: &MarkMotion| {
        let c = Color::from_rgb_u8(one.0, one.1, one.2);
        g.set_agy_flow(ModelRc::new(VecModel::from(vec![c; 10])));
    };
    let want = [one.0, one.1, one.2, 255];
    let (_, flow_n) = icon_ink(&draw(3, M::RainbowFlow, &red), s, want, 6);
    let (_, own_n) = icon_ink(&draw(3, M::None, &red), s, want, 6);
    assert!(flow_n > 3 * own_n, "{own_n} -> {flow_n}");
    let red_still = |g: &MarkMotion| {
        red(g);
        g.set_still(true);
    };
    assert_eq!(
        icon_ink(&draw(3, M::RainbowFlow, &red_still), s, want, 6).1,
        own_n
    );
}

#[test]
fn the_still_set_stills_everything() {
    let busy = MotionSet {
        still: true,
        spin: 200.0,
        breathe: 0.3,
        run: 0.7,
        flow: 40.0,
    };
    let calm = MotionSet {
        still: true,
        spin: 0.0,
        breathe: 1.0,
        run: 0.0,
        flow: 0.0,
    };
    let cases: [(i32, i32, bool); 8] = [
        (1, 1, false),
        (1, 2, false),
        (1, 3, false),
        (1, 4, false),
        (3, 1, false),
        (2, 2, false),
        (2, 3, true),
        (2, 4, false),
    ];
    for (kind, state, rainbow) in cases {
        let h = bar(2.0, kind, state, WORKING);
        h.comp.set_mark_rainbow(rainbow);
        set_rainbow(&h);
        set_motion(&h, busy);
        let a = h.render();
        set_motion(&h, calm);
        let b = h.render();
        assert_eq!(
            a.rgba, b.rgba,
            "kind {kind} state {state} rainbow {rainbow}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Task 25: the marks on the strip, the sidebar and the +N list.
// ---------------------------------------------------------------------------------------------

const DIM: [u8; 4] = [0x92, 0x90, 0x89, 255];
const ACCENT: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const ACCENT_HI: [u8; 4] = [0xf6, 0xa2, 0x5b, 255];

#[derive(Clone, Copy)]
struct Tab {
    mode: i32,
    active: bool,
    kind: i32,
    state: i32,
    colour: [u8; 4],
    rainbow: bool,
    agent: i32,
    settings: bool,
    loading: bool,
}

impl Default for Tab {
    fn default() -> Self {
        Self {
            mode: 0,
            active: false,
            kind: 0,
            state: 0,
            colour: WORKING,
            rainbow: false,
            agent: 0,
            settings: false,
            loading: false,
        }
    }
}

fn tab_row(i: usize, t: &Tab) -> TabRow {
    TabRow {
        id: format!("t{i}").into(),
        label: format!("tab {i}").into(),
        tip: format!("C:\\work\\t{i}").into(),
        folder: format!("~\\work\\t{i}").into(),
        active: t.active,
        loading: t.loading,
        kept: true,
        mark_kind: t.kind,
        mark_color: col(t.colour),
        mark_state: t.state,
        mark_rainbow: t.rainbow,
        agent: t.agent,
        settings: t.settings,
        prog: -1.0,
        ..TabRow::default()
    }
}

macro_rules! set_any_motion {
    ($h:expr, $m:expr) => {{
        let m: MotionSet = $m;
        let g = $h.comp.global::<MarkMotion>();
        g.set_still(m.still);
        g.set_spin(m.spin);
        g.set_breathe(m.breathe);
        g.set_run(m.run);
        g.set_flow(m.flow);
    }};
}

struct StripRig {
    h: Harness<TabStripPreview>,
    /// Each tab's left edge and width in device pixels.
    xs: Vec<(u32, u32)>,
    s: f32,
}

fn strip_with(s: f32, tabs: &[Tab], geom: Option<StripGeom>) -> StripRig {
    let mut x = 0u32;
    let mut xs = Vec::new();
    let mut rows = Vec::new();
    for (i, t) in tabs.iter().enumerate() {
        let w = ((if t.mode == 0 { 150.0 } else { 40.0 }) * s).round() as u32;
        let mut r = tab_row(i, t);
        r.mode = t.mode;
        r.x = x as i32;
        r.w = w as i32;
        rows.push(r);
        xs.push((x, w));
        x += w;
    }
    let geom = geom.unwrap_or(StripGeom {
        more_x: 0,
        more_w: 0,
        plus_x: (900.0 * s).round() as i32,
        plus_w: (40.0 * s).round() as i32,
        more_count: 0,
        dots: ModelRc::default(),
    });
    let h = Harness::new(s, 1000, 36, move || {
        let c = TabStripPreview::new().unwrap();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_geom(geom);
        c.set_new_tab_title("New tab".into());
        c
    });
    set_any_motion!(h, MotionSet::default());
    StripRig { h, xs, s }
}

fn strip(s: f32, tabs: &[Tab]) -> StripRig {
    strip_with(s, tabs, None)
}

fn hair(s: f32) -> u32 {
    s.floor().max(1.0) as u32
}

/// The strip's own height in device pixels (the window is one hairline taller).
fn strip_h(f: &Frame, s: f32) -> u32 {
    f.h - hair(s)
}

fn strip_cy(s: f32) -> f32 {
    (36.0 * s - hair(s) as f32) / 2.0
}

fn is_ground(p: [u8; 4]) -> bool {
    Frame::near(p, GROUND, 1)
}

fn is_bright(p: [u8; 4]) -> bool {
    (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 90
}

fn dv(s: f32, css: f32) -> u32 {
    (css * s).round() as u32
}

/// The box of tab `i`'s end (the ring or the X), widened, clear of the top rule and the bar.
fn end_region(r: &StripRig, i: usize) -> (u32, u32, u32, u32) {
    let (x, w) = r.xs[i];
    let s = r.s;
    (
        ((x + w) as f32 - 10.0 * s - 16.0 * s - 2.0) as u32,
        (4.0 * s) as u32,
        ((x + w) as f32 - 10.0 * s + 2.0) as u32,
        (30.0 * s) as u32,
    )
}

fn mark_bbox(r: &StripRig, f: &Frame, i: usize) -> Option<(u32, u32, u32, u32)> {
    let b = end_region(r, i);
    f.bbox_where(b.0, b.1, b.2, b.3, |p| !is_ground(p))
}

fn centre_x_of(r: &StripRig, i: usize) -> f32 {
    let (x, w) = r.xs[i];
    (x + w) as f32 - 18.0 * r.s
}

fn count_near(f: &Frame, b: (u32, u32, u32, u32), want: [u8; 4], tol: u8) -> usize {
    f.count_where(b.0, b.1, b.2, b.3, |p| Frame::near(p, want, tol))
}

/// The bar band rows of the strip: [top, bottom).
fn band_rows(f: &Frame, s: f32) -> (u32, u32) {
    let bottom = strip_h(f, s);
    (bottom - bar_rows(s), bottom)
}

#[test]
fn ring_sits_in_the_close_buttons_place_and_the_x_slides_in_after_it() {
    for s in [1.0f32, 1.5, 2.25] {
        let r = strip(
            s,
            &[
                Tab {
                    active: true,
                    ..Tab::default()
                },
                Tab {
                    kind: 1,
                    state: 1,
                    ..Tab::default()
                },
            ],
        );
        let f = r.h.render();
        let bb = mark_bbox(&r, &f, 1).expect("the spinner shows");
        let (cx, cy) = centre_of(bb);
        assert!(
            (cx - centre_x_of(&r, 1)).abs() <= 1.5,
            "cx {cx} want {} at {s}",
            centre_x_of(&r, 1)
        );
        assert!((cy - strip_cy(s)).abs() <= 1.5, "cy {cy} at {s}");

        let (x, w) = r.xs[1];
        let label = |f: &Frame| {
            f.bbox_where(
                x + (30.0 * s) as u32,
                (4.0 * s) as u32,
                x + (104.0 * s) as u32,
                (30.0 * s) as u32,
                is_bright,
            )
            .expect("the label shows")
        };
        let before = label(&f);

        r.h.move_to((x + w / 2) as f32 / s, 18.0);
        r.h.advance(200);
        let g = r.h.render();
        // The ring moved left by the X's width and the gap; the X is where the ring was.
        let end_right = (x + w) as f32 - 10.0 * s;
        let rb = g
            .bbox_where(
                (end_right - 34.0 * s - 2.0) as u32,
                (4.0 * s) as u32,
                (end_right - 18.0 * s) as u32,
                (30.0 * s) as u32,
                |p| !is_ground(p),
            )
            .expect("the ring is still there");
        let (rcx, _) = centre_of(rb);
        assert!(
            (rcx - (end_right - 34.0 * s + 8.0 * s)).abs() <= 2.0,
            "ring cx {rcx} at {s}"
        );
        let x_px = g.count_where(
            (end_right - 16.0 * s) as u32,
            (4.0 * s) as u32,
            end_right as u32,
            (30.0 * s) as u32,
            is_bright,
        );
        assert!(x_px > 0, "X glyph pixels appear right of the ring at {s}");
        let after = label(&g);
        let shift = (centre_of(before).0 - centre_of(after).0).abs();
        assert!(
            (7.0 * s..=11.0 * s).contains(&shift),
            "the name box shrank by the X: shift {shift} at {s}"
        );
    }
}

#[test]
fn an_idle_tab_shows_neither_ring_nor_x() {
    for s in [1.0f32, 2.0] {
        let r = strip(
            s,
            &[
                Tab {
                    active: true,
                    ..Tab::default()
                },
                Tab::default(),
            ],
        );
        let f = r.h.render();
        assert!(mark_bbox(&r, &f, 1).is_none(), "nothing at the end at {s}");
    }
}

#[test]
fn a_background_tab_with_a_finished_ring_shows_the_check_in_the_names_ink() {
    for s in [1.0f32, 2.25] {
        let both = Tab {
            kind: 1,
            state: 3,
            colour: FINISHED,
            ..Tab::default()
        };
        let r = strip(
            s,
            &[
                Tab {
                    active: true,
                    ..both
                },
                both,
            ],
        );
        let f = r.h.render();
        let brightest = |i: usize| {
            let b = end_region(&r, i);
            let sum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
            let mut best = [0u8; 4];
            for y in b.1..b.3 {
                for x in b.0..b.2 {
                    let p = f.px(x, y);
                    if sum(p) > sum(best) {
                        best = p;
                    }
                }
            }
            best
        };
        assert!(
            Frame::near(brightest(0), INK, 24),
            "active check is Theme.text at {s}: {:?}",
            brightest(0)
        );
        assert!(
            Frame::near(brightest(1), DIM, 24),
            "background check is Theme.dim at {s}: {:?}",
            brightest(1)
        );
        assert_eq!(
            count_near(&f, end_region(&r, 1), FINISHED, 20),
            0,
            "the check never wears the palette at {s}"
        );
    }
}

#[test]
fn the_bar_sits_along_the_foot_of_each_marked_tab() {
    let s = 1.5;
    let run = Tab {
        kind: 3,
        state: 1,
        ..Tab::default()
    };
    let r = strip(
        s,
        &[
            run,
            run,
            run,
            Tab {
                kind: 2,
                state: 3,
                colour: FINISHED,
                ..Tab::default()
            },
            Tab::default(),
        ],
    );
    let f = r.h.render();
    let (top, bottom) = band_rows(&f, s);
    let h = hair(s);
    for i in 0..3 {
        let (x, w) = r.xs[i];
        assert!(
            count_near(&f, (x, top, x + w - h, bottom), WORKING, 2) > 0,
            "run bar {i} at its foot"
        );
        assert_eq!(
            count_near(&f, (x, 0, x + w - h, top), WORKING, 8),
            0,
            "run bar {i} stays in its band"
        );
    }
    let (x, w) = r.xs[3];
    assert_eq!(
        f.run_len_h(bottom - 1, x, |p| p == FINISHED),
        w - h,
        "the finished line spans the tab"
    );
    let (x, w) = r.xs[4];
    assert_eq!(
        f.count_where(x, top, x + w - h, bottom, |p| !is_ground(p)),
        0,
        "the idle tab has no band"
    );
}

#[test]
fn the_divider_draws_over_the_bar() {
    for s in [1.0f32, 1.5, 2.25] {
        let r = strip(
            s,
            &[Tab {
                kind: 2,
                state: 3,
                colour: FINISHED,
                ..Tab::default()
            }],
        );
        let f = r.h.render();
        let (_, bottom) = band_rows(&f, s);
        let (x, w) = r.xs[0];
        let over = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * 18.0 / 255.0).round() as u8;
        let want = [over(FINISHED[0]), over(FINISHED[1]), over(FINISHED[2]), 255];
        let last = f.px(x + w - 1, bottom - 1);
        assert!(
            Frame::near(last, want, 3),
            "divider over bar {last:?} at {s}"
        );
        assert!(!Frame::near(last, FINISHED, 1), "not the bar colour at {s}");
        let before = f.px(x + w - hair(s) - 1, bottom - 1);
        assert!(
            Frame::near(before, FINISHED, 1),
            "bar up to the divider at {s}"
        );
    }
}

#[test]
fn the_active_tabs_top_rule_and_the_bar_coexist() {
    for s in [1.0f32, 1.5, 2.25] {
        let r = strip(
            s,
            &[Tab {
                active: true,
                kind: 2,
                state: 2,
                colour: QUESTION,
                ..Tab::default()
            }],
        );
        let f = r.h.render();
        let xm = r.xs[0].1 / 2;
        assert_eq!(
            f.run_len_v(xm, 0, |p| Frame::near(p, ACCENT_HI, 2)),
            (2.0 * s).round() as u32,
            "the top rule at {s}"
        );
        let (top, bottom) = band_rows(&f, s);
        for y in top..bottom {
            assert!(
                Frame::near(f.px(xm, y), QUESTION, 2),
                "the bottom line at row {y} at {s}"
            );
        }
    }
}

/// A bounding box of the matching pixels within a circle.
fn bbox_in_circle(
    f: &Frame,
    cx: f32,
    cy: f32,
    r: f32,
    pred: impl Fn([u8; 4]) -> bool,
) -> Option<(u32, u32, u32, u32)> {
    let mut out: Option<(u32, u32, u32, u32)> = None;
    for y in (cy - r).floor().max(0.0) as u32..((cy + r).ceil() as u32).min(f.h) {
        for x in (cx - r).floor().max(0.0) as u32..((cx + r).ceil() as u32).min(f.w) {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r && pred(f.px(x, y)) {
                out = Some(match out {
                    None => (x, y, x, y),
                    Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                });
            }
        }
    }
    out
}

#[test]
fn an_icon_tab_has_the_bar_and_the_corner_disc() {
    const BG: [u8; 4] = [0x30, 0x30, 0x30, 255];
    for s in [1.0f32, 1.5, 2.25] {
        // Ring: the 15 px disc (Theme.tabs) with the spinner centred in it.
        let r = strip(
            s,
            &[Tab {
                mode: 1,
                active: true,
                kind: 1,
                state: 1,
                ..Tab::default()
            }],
        );
        r.h.comp
            .global::<Theme>()
            .set_tab_active(Color::from_rgb_u8(0x30, 0x30, 0x30));
        let f = r.h.render();
        let w = r.xs[0].1 as f32;
        let (dcx, dcy) = (w - (2.0 + 7.5) * s, 10.5 * s);
        let disc = bbox_in_circle(&f, dcx, dcy, 8.5 * s, |p| Frame::near(p, GROUND, 2))
            .expect("the disc shows");
        assert!(
            ((disc.2 - disc.0 + 1) as f32 - 15.0 * s).abs() <= 2.0,
            "the disc is 15 css px at {s}: {disc:?}"
        );
        // The spinner is found inside the disc proper, clear of its anti-aliased rim.
        let (mx, my) = centre_of(disc);
        let spin = bbox_in_circle(&f, mx, my, 6.9 * s, |p| {
            !Frame::near(p, GROUND, 6) && !Frame::near(p, BG, 6)
        })
        .expect("the spinner shows in the disc");
        assert!(
            ((spin.2 - spin.0 + 1) as f32 - 12.0 * s).abs() <= 1.0,
            "the spinner is 12 css px at {s}: {spin:?}"
        );
        let (cx, cy) = centre_of(spin);
        assert!((cx - mx).abs() <= 1.5, "spinner cx {cx} want {mx} at {s}");
        assert!((cy - my).abs() <= 1.5, "spinner cy {cy} want {my} at {s}");

        // Bar: no disc, the line across the foot.
        let r = strip(
            s,
            &[Tab {
                mode: 1,
                active: true,
                kind: 2,
                state: 3,
                colour: FINISHED,
                ..Tab::default()
            }],
        );
        r.h.comp
            .global::<Theme>()
            .set_tab_active(Color::from_rgb_u8(0x30, 0x30, 0x30));
        let f = r.h.render();
        assert!(
            bbox_in_circle(&f, dcx, dcy, 8.5 * s, |p| Frame::near(p, GROUND, 2)).is_none(),
            "no disc under the Bar indicator at {s}"
        );
        let (_, bottom) = band_rows(&f, s);
        assert_eq!(
            f.run_len_h(bottom - 1, 0, |p| p == FINISHED),
            r.xs[0].1 - hair(s),
            "the line crosses the icon tab at {s}"
        );
    }
}

// The sidebar -------------------------------------------------------------------------------

fn side(s: f32, tabs: &[Tab]) -> Harness<TabSidebarPreview> {
    let rows: Vec<TabRow> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| tab_row(i, t))
        .collect();
    let h = Harness::new(s, 700, 500, move || {
        let c = TabSidebarPreview::new().unwrap();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c
    });
    set_any_motion!(h, MotionSet::default());
    h.advance(0);
    h
}

#[test]
fn the_sidebar_row_has_the_bar_clipped_by_its_corners_and_the_ring_at_its_end() {
    for s in [1.0f32, 1.5, 2.25] {
        let h = side(
            s,
            &[
                Tab {
                    kind: 3,
                    state: 1,
                    ..Tab::default()
                },
                Tab {
                    kind: 1,
                    state: 3,
                    colour: FINISHED,
                    ..Tab::default()
                },
            ],
        );
        // Still, the run is a full line: its ends meet the card's corners.
        set_any_motion!(
            h,
            MotionSet {
                still: true,
                ..MotionSet::default()
            }
        );
        let f = h.render();
        // Row 0: css y 50..96, x 8..212.
        let bottom = dv(s, 96.0);
        let rows = bar_rows(s);
        let xin = dv(s, 20.0);
        let inside = f.px(xin, bottom - 1);
        assert!(
            Frame::near(inside, WORKING, 3),
            "bar inside the card {inside:?} at {s}"
        );
        let top_of_band = f.px(xin, bottom - rows);
        assert!(
            Frame::near(top_of_band, WORKING, 3),
            "band is {rows} rows at {s}"
        );
        let above = f.px(xin, bottom - rows - 1);
        assert!(
            !Frame::near(above, WORKING, 20),
            "band stops at {rows} rows at {s}"
        );
        // The corner pixels are exactly what the same card shows without a mark.
        let bare = side(s, &[Tab::default(), Tab::default()]).render();
        for x in [dv(s, 8.0), dv(s, 9.0), dv(s, 212.0) - 1, dv(s, 211.0) - 1] {
            for y in [bottom - 1, bottom - rows] {
                assert_eq!(
                    f.px(x, y),
                    bare.px(x, y),
                    "no bar pixel outside the rounded corner at ({x}, {y}) at {s}"
                );
            }
        }
        // And the bar does reach the curve's end.
        let reach = f.px(dv(s, 8.0) + dv(s, 9.0), bottom - 1);
        assert!(
            Frame::near(reach, WORKING, 3),
            "the bar reaches {reach:?} at {s}"
        );
        // Row 1: css y 100..146; the check sits in the end box (css x 186..204).
        let b = (dv(s, 184.0), dv(s, 102.0), dv(s, 206.0), dv(s, 144.0));
        let bb = f
            .bbox_where(b.0, b.1, b.2, b.3, is_bright)
            .expect("the check shows at the end");
        let (cx, cy) = centre_of(bb);
        assert!((cx - 195.0 * s).abs() <= 1.5, "check cx {cx} at {s}");
        assert!((cy - 123.0 * s).abs() <= 1.5, "check cy {cy} at {s}");
    }
}

fn icon_counts(f: &Frame, r: (u32, u32, u32, u32)) -> (usize, usize) {
    (count_near(f, r, CLAWD, 4), count_near(f, r, INK, 10))
}

#[test]
fn the_agent_icon_follows_the_agent() {
    let tabs = [
        Tab {
            agent: 0,
            ..Tab::default()
        },
        Tab {
            agent: 1,
            ..Tab::default()
        },
        Tab {
            agent: 2,
            ..Tab::default()
        },
        Tab {
            settings: true,
            ..Tab::default()
        },
    ];
    for s in [1.0f32, 1.5, 2.25] {
        for active in [false, true] {
            let mut t = tabs;
            t[1].active = active;
            let r = strip(s, &t);
            let f = r.h.render();
            for i in 0..4 {
                let (x, _) = r.xs[i];
                let reg = (x + dv(s, 8.0), dv(s, 4.0), x + dv(s, 30.0), dv(s, 32.0));
                let (clawd, _) = icon_counts(&f, reg);
                let any = f.count_where(reg.0, reg.1, reg.2, reg.3, |p| !is_ground(p));
                assert!(any > 0, "icon {i} shows in the strip at {s}");
                if i == 1 {
                    assert!(clawd > 0, "Clawd stays orange (active {active}) at {s}");
                } else {
                    assert_eq!(clawd, 0, "icon {i} is not Clawd at {s}");
                }
            }
        }
        // The sidebar: the icon band, css x 8..54 of each 46 px row.
        let h = side(s, &tabs);
        let f = h.render();
        for i in 0..4usize {
            let y0 = 50.0 + 50.0 * i as f32;
            let reg = (dv(s, 20.0), dv(s, y0 + 4.0), dv(s, 42.0), dv(s, y0 + 42.0));
            let (clawd, ink) = icon_counts(&f, reg);
            if i == 1 {
                assert!(clawd > 0, "Clawd in the sidebar at {s}");
                assert_eq!(ink, 0, "Clawd wears no ink in the sidebar at {s}");
            } else {
                assert_eq!(clawd, 0, "icon {i} is not Clawd in the sidebar at {s}");
                assert!(ink > 0, "icon {i} wears the ink in the sidebar at {s}");
            }
        }
    }
}

#[test]
fn the_plus_n_dots_wear_the_mark_colours() {
    for s in [1.0f32, 1.5, 2.25] {
        let colours = [WORKING, QUESTION, FINISHED, FAILED, WORKING];
        let geom = StripGeom {
            more_x: dv(s, 400.0) as i32,
            more_w: dv(s, 90.0) as i32,
            plus_x: dv(s, 494.0) as i32,
            plus_w: dv(s, 40.0) as i32,
            more_count: 5,
            dots: ModelRc::new(VecModel::from(
                colours.iter().map(|c| col(*c)).collect::<Vec<_>>(),
            )),
        };
        let r = strip_with(s, &[Tab::default()], Some(geom));
        let f = r.h.render();
        let y = (18.0 * s) as u32;
        let which = |p: [u8; 4]| {
            [WORKING, QUESTION, FINISHED, FAILED]
                .iter()
                .position(|c| Frame::near(p, *c, 2))
        };
        // Runs of dot-coloured pixels along the middle row, in order.
        let mut runs: Vec<(usize, u32)> = Vec::new();
        for x in dv(s, 400.0)..dv(s, 490.0) {
            if let Some(k) = which(f.px(x, y)) {
                match runs.last_mut() {
                    Some(last) if last.0 == k && x > 0 && which(f.px(x - 1, y)) == Some(k) => {
                        last.1 += 1;
                    }
                    _ => runs.push((k, 1)),
                }
            }
        }
        let order: Vec<usize> = runs.iter().map(|r| r.0).collect();
        assert_eq!(
            order,
            vec![0, 1, 2, 3],
            "four dots, in order, at {s}: {runs:?}"
        );
        for (_, len) in &runs {
            assert!(
                (3.5 * s..=6.0 * s + 1.0).contains(&(*len as f32)),
                "a dot is up to 6 css px at {s}: {len}"
            );
        }
    }
}

// The +N list and the tip -------------------------------------------------------------------

fn list_row(mark_state: i32, agent: i32) -> OverflowRow {
    OverflowRow {
        id: "t9".into(),
        label: "tab nine".into(),
        folder: "C:\\work".into(),
        needs: false,
        loading: false,
        settings: false,
        mark_state,
        mark_color: Color::from_argb_u8(0, 0, 0, 0),
        agent,
    }
}

fn list_harness(s: f32, row: OverflowRow) -> Harness<MenusPreview> {
    let h = Harness::new(s, 400, 300, || MenusPreview::new().unwrap());
    h.comp.set_ov_rows(ModelRc::new(VecModel::from(vec![row])));
    h.comp.set_ov_right(450.0);
    h.comp.set_ov_bottom(36.0);
    h.comp.set_ov_open(true);
    h.advance(0);
    set_any_motion!(h, MotionSet::default());
    h
}

fn diff_bbox(a: &Frame, b: &Frame) -> Option<(u32, u32, u32, u32)> {
    let mut out: Option<(u32, u32, u32, u32)> = None;
    for y in 0..a.h {
        for x in 0..a.w {
            if a.px(x, y) != b.px(x, y) {
                out = Some(match out {
                    None => (x, y, x, y),
                    Some((p, q, r, t)) => (p.min(x), q.min(y), r.max(x), t.max(y)),
                });
            }
        }
    }
    out
}

#[test]
fn a_marked_list_row_shows_its_ring_and_icon() {
    for s in [1.0f32, 1.5, 2.25] {
        let plain = list_harness(s, list_row(0, 0)).render();
        let h = list_harness(s, list_row(1, 0));
        let marked = h.render();
        let d = diff_bbox(&plain, &marked).expect("the spinner is drawn at the row's end");
        let (w, hgt) = ((d.2 - d.0 + 1) as f32, (d.3 - d.1 + 1) as f32);
        assert!(
            (w - 12.0 * s).abs() <= 2.0 * s.max(1.0),
            "ring wide {w} at {s}"
        );
        assert!(
            (hgt - 12.0 * s).abs() <= 2.0 * s.max(1.0),
            "ring high {hgt} at {s}"
        );
        let box_x = h.comp.get_ov_box_x() * s;
        let box_w = h.comp.get_ov_box_w() * s;
        assert!(
            d.0 as f32 > box_x + box_w / 2.0,
            "the ring is at the row's end: {d:?} in box {box_x} + {box_w}"
        );
        // Claude's row wears Clawd's orange.
        let clawd = list_harness(s, list_row(0, 1)).render();
        assert!(
            count_near(&clawd, (0, 0, clawd.w, clawd.h), CLAWD, 4) > 0,
            "the list row shows the agent icon at {s}"
        );
        assert_eq!(
            count_near(&plain, (0, 0, plain.w, plain.h), CLAWD, 4),
            0,
            "a shell row shows no Clawd at {s}"
        );
    }
}

#[test]
fn a_failed_tip_has_two_lines() {
    // Placement (centred under the tab) is M3's TipHost rule and does not change; this is the box.
    let one = Harness::new(1.0, 400, 300, || MenusPreview::new().unwrap());
    one.comp.set_tip_text("C:\\a".into());
    one.comp.set_tip_visible(true);
    one.advance(200);
    let two = Harness::new(1.0, 400, 300, || MenusPreview::new().unwrap());
    two.comp.set_tip_text("C:\\a\nFailed: rate limit".into());
    two.comp.set_tip_visible(true);
    two.advance(200);
    let grow = two.comp.get_tip_h() - one.comp.get_tip_h();
    assert!(
        (10.0..=24.0).contains(&grow),
        "the box grows by about one line: {} vs {}",
        two.comp.get_tip_h(),
        one.comp.get_tip_h()
    );
    assert!(two.comp.get_tip_w() >= one.comp.get_tip_w());
    // Two lines are drawn: two runs of rows holding ink inside the bubble.
    let f = two.render();
    let (bx, by) = (20u32, 20u32);
    let (bw, bh) = (two.comp.get_tip_w() as u32, two.comp.get_tip_h() as u32);
    let mut lines = 0;
    let mut prev = false;
    for y in by + 3..by + bh - 3 {
        let ink = f.count_where(bx + 6, y, bx + bw - 6, y + 1, is_bright) > 0;
        if ink && !prev {
            lines += 1;
        }
        prev = ink;
    }
    assert_eq!(lines, 2, "two separate text lines");
}

// The one clock and the empty state ---------------------------------------------------------

fn accent_near(f: &Frame, x: f32, y: f32) -> bool {
    let (x, y) = (x.round() as i64, y.round() as i64);
    (-1..=1).any(|dy| {
        (-1..=1).any(|dx| {
            let (px, py) = (x + dx, y + dy);
            px >= 0
                && py >= 0
                && (px as u32) < f.w
                && (py as u32) < f.h
                && Frame::near(f.px(px as u32, py as u32), ACCENT, 40)
        })
    })
}

#[test]
fn the_loading_ring_uses_the_one_clock() {
    let s = 2.25;
    let r = strip(
        s,
        &[
            Tab {
                active: true,
                ..Tab::default()
            },
            Tab {
                loading: true,
                ..Tab::default()
            },
        ],
    );
    let cx = centre_x_of(&r, 1);
    let cy = strip_cy(s);
    let rad = 3.75 * s;
    let probe = |m: MotionSet| {
        set_any_motion!(r.h, m);
        let f = r.h.render();
        (
            accent_near(&f, cx, cy - rad),
            accent_near(&f, cx + rad, cy),
            accent_near(&f, cx, cy + rad),
            accent_near(&f, cx - rad, cy),
        )
    };
    // (top, right, bottom, left)
    assert_eq!(
        probe(MotionSet::default()),
        (true, false, false, false),
        "spin 0 puts the arc on top"
    );
    assert_eq!(
        probe(MotionSet {
            spin: 90.0,
            ..MotionSet::default()
        }),
        (false, true, false, false),
        "spin 90 puts the arc on the right"
    );
    assert_eq!(
        probe(MotionSet {
            still: true,
            spin: 200.0,
            ..MotionSet::default()
        }),
        (true, true, false, false),
        "still gives the two-sided arc"
    );
}

#[test]
fn no_mark_is_drawn_for_an_empty_state() {
    for s in [1.0f32, 2.0] {
        let stale = |mode: i32| Tab {
            mode,
            state: 3,
            ..Tab::default()
        };
        let clean = |mode: i32| Tab {
            mode,
            ..Tab::default()
        };
        let shown = |tabs: &[Tab]| {
            let r = strip(s, tabs);
            (r.h.render(), r.xs.clone())
        };
        let (a, xs) = shown(&[clean(0), stale(0), stale(1)]);
        let (b, _) = shown(&[clean(0), clean(0), clean(1)]);
        // A stale state with kind 0 renders exactly what an empty tab renders.
        assert_eq!(a.rgba, b.rgba, "kind 0 draws nothing at {s}");
        let (top, bottom) = band_rows(&a, s);
        let (x, w) = xs[1];
        assert_eq!(
            a.count_where(x, top, x + w - hair(s), bottom, |p| !is_ground(p)),
            0,
            "no bar at {s}"
        );
    }
}

#[test]
fn every_mark_works_at_the_eight_scales() {
    for s in SCALES {
        let r = strip(
            s,
            &[
                Tab {
                    active: true,
                    ..Tab::default()
                },
                Tab {
                    kind: 1,
                    state: 1,
                    agent: 1,
                    ..Tab::default()
                },
                Tab {
                    kind: 2,
                    state: 3,
                    colour: FINISHED,
                    ..Tab::default()
                },
            ],
        );
        let f = r.h.render();
        // The ring: 12 css px, centred in the end box.
        let bb = mark_bbox(&r, &f, 1).expect("the ring shows");
        let size = 12.0 * s;
        assert!(
            ((bb.2 - bb.0 + 1) as f32 - size).abs() <= 1.0,
            "ring wide at {s}: {bb:?}"
        );
        assert!(
            ((bb.3 - bb.1 + 1) as f32 - size).abs() <= 1.0,
            "ring high at {s}: {bb:?}"
        );
        let (cx, cy) = centre_of(bb);
        assert!((cx - centre_x_of(&r, 1)).abs() <= 1.5, "ring cx at {s}");
        assert!((cy - strip_cy(s)).abs() <= 1.5, "ring cy at {s}");
        // The bar: bar_rows tall along the foot.
        let (top, bottom) = band_rows(&f, s);
        let (x, w) = r.xs[2];
        let xm = x + w / 2;
        assert_eq!(
            f.run_len_v(xm, top, |p| p == FINISHED),
            bottom - top,
            "bar rows at {s}"
        );
        assert!(
            !Frame::near(f.px(xm, top - 1), FINISHED, 2),
            "the row above the bar at {s}"
        );
        // Clawd: within 18 css px, centred left of the name.
        let (x1, _) = r.xs[1];
        let reg = (x1 + dv(s, 8.0), dv(s, 4.0), x1 + dv(s, 30.0), dv(s, 32.0));
        let cb = f
            .bbox_where(reg.0, reg.1, reg.2, reg.3, |p| Frame::near(p, CLAWD, 40))
            .expect("Clawd shows");
        assert!(
            ((cb.2 - cb.0 + 1) as f32) <= 18.0 * s + 1.0
                && ((cb.3 - cb.1 + 1) as f32) <= 18.0 * s + 1.0,
            "Clawd fits 18 css px at {s}: {cb:?}"
        );
        let (ccx, ccy) = centre_of(cb);
        assert!(
            (ccx - (x1 as f32 + 19.0 * s)).abs() <= 2.0 * s.max(1.0),
            "Clawd cx at {s}"
        );
        assert!(
            (ccy - strip_cy(s)).abs() <= 2.0 * s.max(1.0),
            "Clawd cy at {s}"
        );
    }
}
