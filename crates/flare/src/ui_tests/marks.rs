//! Mark component tests at the eight scales (Tasks 24 and 25).

use flare_agent::mark_motion::run_bar_x;
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{MarkMotion, MarkPreview, Theme};

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
        for kind in 0..4 {
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
            } else {
                assert_eq!(clawd_px, 0, "icon {kind} is never Clawd's orange at {s}");
                assert!(ink_px > 0, "icon {kind} is the ink at {s}");
            }
        }
    }
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
