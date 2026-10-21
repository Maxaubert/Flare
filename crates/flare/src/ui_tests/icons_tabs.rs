//! Icon tests for the tab strip and the tab marks (Task 50). Slint fits a stroked Path into its box minus the stroke,
//! so every stroked glyph must be drawn in a box one stroke wider than its nominal size, moved back by half a
//! stroke. These tests measure the drawn ink at all eight scalings against the size the glyph's own commands give.
//! The helpers here (`svg_bbox`, `expected_extent_px`, `Ink`) are shared with the other two icon test files.

use slint::{ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{MarkPreview, StripGeom, TabRow, TabStripPreview};

/// The window ground (`Theme.ground` = `Theme.tabs`), the text ink and the icon ink, as the theme has them.
pub const GROUND: [u8; 4] = [23, 23, 23, 255];
pub const TEXT: [f32; 3] = [222.0, 218.0, 207.0];
pub const ICON: [f32; 3] = [146.0, 144.0, 137.0];
pub const TEXT_SOFT: [f32; 3] = [194.0, 191.0, 181.0];

/// The glyph strings of `glyphs.slint` that the tab strip and the tab marks draw.
pub const CLOSE: &str = "M6 6l12 12M18 6L6 18";
pub const PLUS: &str = "M12 6v12m6-6H6";
pub const CHEVRON_DOWN: &str = "M6 9l6 6 6-6";
pub const CHECK: &str = "M3.2 8.6l3 3 6.6-7.2";
pub const CROSS: &str = "M4 4l8 8M12 4l-8 8";
pub const PROMPT: &str = "M5 7l5 5-5 5M12 17h7";
pub const COG: &str = "M15 12a3 3 0 1 1-6 0a3 3 0 1 1 6 0z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z";
/// The tab ring of `tabs.slint` (LoadingRing) and the mark ring of `tabmark.slint` (MarkRing).
pub const RING_TAB: &str = "M4.5 0.75A3.75 3.75 0 1 1 4.499 0.75Z";
pub const RING_MARK: &str = "M11 6a5 5 0 1 1-10 0a5 5 0 1 1 10 0z";

/// Bounding box `(min x, min y, max x, max y)` of an SVG path in its own viewbox units, for the commands the
/// glyph table uses (`M L H V C A Z`, absolute and relative). Curves and arcs are sampled, so their true
/// extremes are included.
pub fn svg_bbox(commands: &str) -> (f32, f32, f32, f32) {
    let b = commands.as_bytes();
    let mut i = 0usize;
    let mut cmd = b'M';
    let mut cur = (0.0f64, 0.0f64);
    let mut start = (0.0f64, 0.0f64);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    loop {
        skip_sep(b, &mut i);
        if i >= b.len() {
            break;
        }
        if b[i].is_ascii_alphabetic() {
            cmd = b[i];
            i += 1;
        }
        let rel = cmd.is_ascii_lowercase();
        let origin = if rel { cur } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                cur = p;
                start = p;
                pts.push(p);
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let p = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                cur = p;
                pts.push(p);
            }
            b'H' => {
                let v = num(b, &mut i);
                cur = (if rel { cur.0 + v } else { v }, cur.1);
                pts.push(cur);
            }
            b'V' => {
                let v = num(b, &mut i);
                cur = (cur.0, if rel { cur.1 + v } else { v });
                pts.push(cur);
            }
            b'C' => {
                let c1 = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                let c2 = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                let p = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                for k in 1..=32 {
                    let t = k as f64 / 32.0;
                    let u = 1.0 - t;
                    pts.push((
                        u * u * u * cur.0
                            + 3.0 * u * u * t * c1.0
                            + 3.0 * u * t * t * c2.0
                            + t * t * t * p.0,
                        u * u * u * cur.1
                            + 3.0 * u * u * t * c1.1
                            + 3.0 * u * t * t * c2.1
                            + t * t * t * p.1,
                    ));
                }
                cur = p;
            }
            b'A' => {
                let rx = num(b, &mut i).abs();
                let ry = num(b, &mut i).abs();
                let _rotation = num(b, &mut i);
                let large = flag(b, &mut i);
                let sweep = flag(b, &mut i);
                let p = (num(b, &mut i) + origin.0, num(b, &mut i) + origin.1);
                arc(cur, rx, ry, large, sweep, p, &mut pts);
                cur = p;
            }
            b'Z' => {
                cur = start;
                pts.push(start);
            }
            other => panic!("svg_bbox: unsupported path command {}", other as char),
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x0 as f32, y0 as f32, x1 as f32, y1 as f32)
}

fn skip_sep(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b',' | b'\n' | b'\t' | b'\r') {
        *i += 1;
    }
}

fn num(b: &[u8], i: &mut usize) -> f64 {
    skip_sep(b, i);
    let start = *i;
    if *i < b.len() && matches!(b[*i], b'-' | b'+') {
        *i += 1;
    }
    let mut dot = false;
    while *i < b.len() && (b[*i].is_ascii_digit() || (b[*i] == b'.' && !dot)) {
        dot |= b[*i] == b'.';
        *i += 1;
    }
    std::str::from_utf8(&b[start..*i])
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .expect("svg_bbox: a number was expected")
}

/// An arc flag is one character, `0` or `1`, possibly with no separator before the next number.
fn flag(b: &[u8], i: &mut usize) -> bool {
    skip_sep(b, i);
    let v = b[*i] == b'1';
    *i += 1;
    v
}

/// SVG's endpoint-to-centre arc conversion (F.6.5, no rotation), sampled into `out`.
fn arc(
    p1: (f64, f64),
    rx: f64,
    ry: f64,
    large: bool,
    sweep: bool,
    p2: (f64, f64),
    out: &mut Vec<(f64, f64)>,
) {
    out.push(p1);
    if (p1.0 == p2.0 && p1.1 == p2.1) || rx == 0.0 || ry == 0.0 {
        out.push(p2);
        return;
    }
    let (mut rx, mut ry) = (rx, ry);
    let dx = (p1.0 - p2.0) / 2.0;
    let dy = (p1.1 - p2.1) / 2.0;
    let lambda = dx * dx / (rx * rx) + dy * dy / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
    let den = rx * rx * dy * dy + ry * ry * dx * dx;
    let mut coef = (num.max(0.0) / den).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * dy / ry;
    let cyp = coef * -ry * dx / rx;
    let cx = cxp + (p1.0 + p2.0) / 2.0;
    let cy = cyp + (p1.1 + p2.1) / 2.0;
    let u = ((dx - cxp) / rx, (dy - cyp) / ry);
    let v = ((-dx - cxp) / rx, (-dy - cyp) / ry);
    let theta1 = u.1.atan2(u.0);
    let mut dtheta = (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f64::consts::TAU;
    }
    if sweep && dtheta < 0.0 {
        dtheta += std::f64::consts::TAU;
    }
    const STEPS: usize = 720;
    for k in 1..=STEPS {
        let th = theta1 + dtheta * k as f64 / STEPS as f64;
        out.push((cx + rx * th.cos(), cy + ry * th.sin()));
    }
}

/// The drawn size of a stroked glyph in device pixels: the glyph's own bounding box scaled from its viewbox to
/// the nominal `box_px`, plus one stroke (round caps and joins add half a stroke at each end), all times `scale`.
/// `box_px` is the nominal size, never the grown box.
pub fn expected_extent_px(
    commands: &str,
    box_px: f32,
    viewbox: f32,
    stroke_px: f32,
    scale: f32,
) -> (f32, f32) {
    let (x0, y0, x1, y1) = svg_bbox(commands);
    (
        ((x1 - x0) * box_px / viewbox + stroke_px) * scale,
        ((y1 - y0) * box_px / viewbox + stroke_px) * scale,
    )
}

/// A pixel is ink when it is at least half way from the background to the stroke colour.
pub struct Ink {
    bg: [f32; 3],
    d: [f32; 3],
    dd: f32,
}

impl Ink {
    pub fn new(bg: [u8; 4], stroke: [f32; 3]) -> Self {
        let bgf = [f32::from(bg[0]), f32::from(bg[1]), f32::from(bg[2])];
        let d = [stroke[0] - bgf[0], stroke[1] - bgf[1], stroke[2] - bgf[2]];
        Self {
            bg: bgf,
            d,
            dd: d[0] * d[0] + d[1] * d[1] + d[2] * d[2],
        }
    }

    /// The share of the pixel's colour that is ink, from 0 (ground) to 1 (full stroke colour).
    pub fn cov(&self, p: [u8; 4]) -> f32 {
        let q = [
            f32::from(p[0]) - self.bg[0],
            f32::from(p[1]) - self.bg[1],
            f32::from(p[2]) - self.bg[2],
        ];
        ((q[0] * self.d[0] + q[1] * self.d[1] + q[2] * self.d[2]) / self.dd).clamp(0.0, 1.0)
    }

    pub fn on(&self, p: [u8; 4]) -> bool {
        self.cov(p) >= 0.5
    }
}

/// The colour a stroke of `ink` at `alpha` draws over `bg`.
pub fn blend(bg: [u8; 4], ink: [f32; 3], alpha: f32) -> [f32; 3] {
    std::array::from_fn(|c| f32::from(bg[c]) + alpha * (ink[c] - f32::from(bg[c])))
}

/// A column or row whose best pixel holds less ink than this is treated as empty (8-bit colour noise).
const INK_FLOOR: f32 = 0.05;

/// The ink edges `(x0, y0, x1, y1)` in device pixels (right and bottom exclusive) inside the box `region`, to a
/// fraction of a pixel. Each edge is the outermost column (or row) that holds ink, moved in by the share of that
/// column the ink does not cover, so a flat edge reads exactly. A whole-pixel count (any pixel over half ink)
/// drops the faint tip of a round cap: a 1.1 css stroke's tip never reaches half coverage, and the count reads a
/// pixel short of the drawn size, so the edges are read from coverage instead.
pub fn ink_edges(
    f: &Frame,
    region: (u32, u32, u32, u32),
    ink: &Ink,
) -> Option<(f32, f32, f32, f32)> {
    let (x0, y0) = (region.0, region.1);
    let (x1, y1) = (region.2.min(f.w), region.3.min(f.h));
    let cols: Vec<f32> = (x0..x1)
        .map(|x| (y0..y1).map(|y| ink.cov(f.px(x, y))).fold(0.0, f32::max))
        .collect();
    let rows: Vec<f32> = (y0..y1)
        .map(|y| (x0..x1).map(|x| ink.cov(f.px(x, y))).fold(0.0, f32::max))
        .collect();
    let first = |v: &[f32]| v.iter().position(|&c| c > INK_FLOOR);
    let last = |v: &[f32]| v.iter().rposition(|&c| c > INK_FLOOR);
    let (l, r) = (first(&cols)?, last(&cols)?);
    let (t, b) = (first(&rows)?, last(&rows)?);
    Some((
        (x0 + l as u32) as f32 + 1.0 - cols[l],
        (y0 + t as u32) as f32 + 1.0 - rows[t],
        (x0 + r as u32) as f32 + cols[r],
        (y0 + b as u32) as f32 + rows[b],
    ))
}

/// A box of device pixels around a centre, clamped to the frame.
pub fn region_around(f: &Frame, cx: f32, cy: f32, r: f32) -> (u32, u32, u32, u32) {
    (
        (cx - r).floor().max(0.0) as u32,
        (cy - r).floor().max(0.0) as u32,
        ((cx + r).ceil().max(0.0) as u32).min(f.w),
        ((cy + r).ceil().max(0.0) as u32).min(f.h),
    )
}

/// Asserts the drawn ink is the expected size within one device pixel. The centre is checked by
/// `centre_of_the_glyph_is_unchanged` against the centres of the build before the box growth.
fn assert_size(what: &str, s: f32, got: (f32, f32, f32, f32), want: (f32, f32)) {
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

/// The centre of measured ink edges, in device pixels.
fn centre_of(got: (f32, f32, f32, f32)) -> (f32, f32) {
    ((got.0 + got.2) / 2.0, (got.1 + got.3) / 2.0)
}

fn hair(s: f32) -> f32 {
    s.floor().max(1.0)
}

/// The middle of the tab strip (its height is 36 css px less one hairline), in device pixels.
fn strip_cy(s: f32) -> f32 {
    (36.0 * s - hair(s)) / 2.0
}

fn tab(i: usize, x: i32, w: i32, active: bool, loading: bool) -> TabRow {
    TabRow {
        id: format!("t{i}").into(),
        label: format!("tab {i}").into(),
        tip: format!("C:\\work\\t{i}").into(),
        folder: format!("~\\work\\t{i}").into(),
        active,
        loading,
        kept: true,
        mode: 0,
        x,
        w,
        ..TabRow::default()
    }
}

fn strip(s: f32, rows: Vec<TabRow>, geom: StripGeom) -> Harness<TabStripPreview> {
    Harness::new(s, 1000, 36, move || {
        let c = TabStripPreview::new().unwrap();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_geom(geom);
        c.set_new_tab_title("New tab".into());
        c
    })
}

/// The strip geometry with the + button at its usual place and `more` hidden tabs in the +N button at its left.
fn geom(s: f32, more: i32) -> StripGeom {
    StripGeom {
        more_x: (800.0 * s).round() as i32,
        more_w: (60.0 * s).round() as i32,
        plus_x: (900.0 * s).round() as i32,
        plus_w: (40.0 * s).round() as i32,
        more_count: more,
        dots: ModelRc::default(),
    }
}

/// The centre of a tab's end (the X or the ring) in device pixels: 18 css px in from the tab's right edge.
fn end_centre(s: f32, tab_w: i32) -> (f32, f32) {
    (tab_w as f32 - 18.0 * s, strip_cy(s))
}

#[test]
fn svg_bbox_reads_absolute_and_relative_commands() {
    assert_eq!(svg_bbox(CLOSE), (6.0, 6.0, 18.0, 18.0));
    assert_eq!(svg_bbox(PLUS), (6.0, 6.0, 18.0, 18.0));
    assert_eq!(svg_bbox(CHEVRON_DOWN), (6.0, 9.0, 18.0, 15.0));
    assert_eq!(svg_bbox(PROMPT), (5.0, 7.0, 19.0, 17.0));
}

#[test]
fn svg_bbox_follows_an_arc_to_its_extremes() {
    let (x0, y0, x1, y1) = svg_bbox(RING_TAB);
    for v in [x0, y0] {
        assert!((v - 0.75).abs() < 1e-3, "ring starts at 0.75, got {v}");
    }
    for v in [x1, y1] {
        assert!((v - 8.25).abs() < 1e-3, "ring ends at 8.25, got {v}");
    }
}

#[test]
fn expected_extent_adds_one_stroke_at_the_nominal_size() {
    // A 12 unit cross at nominal 10 px, stroke 1 px, at 2x: (12 * 10 / 24 + 1) * 2.
    let (w, h) = expected_extent_px(CLOSE, 10.0, 24.0, 1.0, 2.0);
    assert!(
        (w - 12.0).abs() < 1e-4 && (h - 12.0).abs() < 1e-4,
        "got {w} {h}"
    );
}

/// A measured tab-strip glyph: its ink edges in device pixels, and the size it should draw at (from its own
/// commands, over the nominal box, plus one stroke).
type Drawn = ((f32, f32, f32, f32), (f32, f32));

fn draw_plus(s: f32) -> Drawn {
    let h = strip(s, Vec::new(), geom(s, 0));
    let f = h.render();
    let (x0, x1) = (
        (900.0 * s).round(),
        (900.0 * s).round() + (40.0 * s).round(),
    );
    let region = (
        x0 as u32,
        (4.0 * s) as u32,
        x1 as u32,
        (strip_cy(s) * 2.0 - 4.0 * s) as u32,
    );
    let got = ink_edges(&f, region, &Ink::new(GROUND, ICON)).expect("the + is drawn");
    (
        got,
        expected_extent_px(PLUS, 13.0, 24.0, 2.0 * 13.0 / 24.0, s),
    )
}

fn draw_close(s: f32) -> Drawn {
    let w = (150.0 * s).round() as i32;
    let h = strip(s, vec![tab(0, 0, w, true, false)], geom(s, 0));
    let f = h.render();
    let (cx, cy) = end_centre(s, w);
    let region = region_around(&f, cx, cy, 9.0 * s);
    let bg = f.px(region.0, region.1);
    let got = ink_edges(&f, region, &Ink::new(bg, ICON)).expect("the X is drawn");
    let glyph = 10.0;
    (
        got,
        expected_extent_px(CLOSE, glyph, 24.0, 2.4 * glyph / 24.0, s),
    )
}

fn draw_ring(s: f32) -> Drawn {
    let w = (150.0 * s).round() as i32;
    let h = strip(s, vec![tab(0, 0, w, false, true)], geom(s, 0));
    let f = h.render();
    let (cx, cy) = end_centre(s, w);
    let region = region_around(&f, cx, cy, 9.0 * s);
    let bg = f.px(region.0, region.1);
    // The track is the text ink at 22 percent; the accent arc sits on the same circle.
    let track = blend(bg, TEXT, 0.22);
    let got = ink_edges(&f, region, &Ink::new(bg, track)).expect("the ring is drawn");
    (got, expected_extent_px(RING_TAB, 9.0, 9.0, 1.5, s))
}

fn draw_chevron(s: f32) -> Drawn {
    let h = strip(s, Vec::new(), geom(s, 3));
    let f = h.render();
    let (x0, x1) = (
        (800.0 * s).round() as u32,
        ((800.0 + 60.0) * s).round() as u32,
    );
    let region = (
        x0,
        (4.0 * s) as u32,
        x1,
        (strip_cy(s) * 2.0 - 4.0 * s) as u32,
    );
    let bg = f.px(region.0, region.1);
    let ink = Ink::new(bg, TEXT_SOFT);
    // The +3 text sits left of the chevron, with a six pixel gap: the chevron is the rightmost cluster of ink.
    let got = rightmost_cluster(&f, region, &ink).expect("the chevron is drawn");
    (
        got,
        expected_extent_px(CHEVRON_DOWN, 11.0, 24.0, 2.2 * 11.0 / 24.0, s),
    )
}

#[test]
fn plus_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let (got, want) = draw_plus(s);
        assert_size("plus", s, got, want);
    }
}

#[test]
fn close_x_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let (got, want) = draw_close(s);
        assert_size("close X", s, got, want);
    }
}

#[test]
fn loading_ring_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let (got, want) = draw_ring(s);
        assert_size("loading ring", s, got, want);
    }
}

#[test]
fn chevron_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let (got, want) = draw_chevron(s);
        assert_size("chevron", s, got, want);
    }
}

/// The four tab-strip glyphs keep the centre they had before their boxes grew. The centres are device pixels,
/// recorded from the build before the change, in `SCALES` order, and each must hold within half a pixel.
#[test]
fn centre_of_the_glyph_is_unchanged() {
    type Measure = fn(f32) -> Drawn;
    type Centres = [(f32, f32); 8];
    let before: [(&str, Measure, Centres); 4] = [
        (
            "plus",
            draw_plus,
            [
                (920.0, 17.5),
                (1150.0, 22.0),
                (1380.0, 26.5),
                (1610.0, 31.0),
                (1840.0, 35.0),
                (2070.0, 39.5),
                (2300.0, 44.0),
                (2760.0, 52.5),
            ],
        ),
        (
            "close X",
            draw_close,
            [
                (132.0, 17.5),
                (165.5, 22.0),
                (198.0, 26.5),
                (231.5, 31.0),
                (264.0, 35.0),
                (297.5, 39.5),
                (330.0, 44.0),
                (396.0, 52.5),
            ],
        ),
        (
            "loading ring",
            draw_ring,
            [
                (132.0, 17.15),
                (165.5, 21.89),
                (198.0, 26.17),
                (231.5, 30.86),
                (264.0, 34.83),
                (297.5, 39.25),
                (330.0, 43.8),
                (396.0, 52.4),
            ],
        ),
        (
            "chevron",
            draw_chevron,
            [
                (840.5, 17.53),
                (1050.64, 21.9),
                (1260.74, 26.23),
                (1470.86, 30.58),
                (1681.0, 35.0),
                (1891.12, 39.34),
                (2101.25, 43.71),
                (2521.5, 52.55),
            ],
        ),
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

/// The rightmost run of columns holding ink, as ink edges.
fn rightmost_cluster(
    f: &Frame,
    region: (u32, u32, u32, u32),
    ink: &Ink,
) -> Option<(f32, f32, f32, f32)> {
    let col_has = |x: u32| (region.1..region.3).any(|y| ink.on(f.px(x, y)));
    let mut xr = region.2.min(f.w);
    while xr > region.0 && !col_has(xr - 1) {
        xr -= 1;
    }
    if xr == region.0 {
        return None;
    }
    let mut xl = xr - 1;
    while xl > region.0 && col_has(xl - 1) {
        xl -= 1;
    }
    // The cluster stops at its first column over half ink; the faint tips beside it are read too (the gap to the
    // +N text is six pixels, so two columns on each side stay clear of it).
    let (xl, xr) = (xl.saturating_sub(2).max(region.0), (xr + 2).min(region.2));
    ink_edges(f, (xl, region.1, xr, region.3), ink)
}

/// The mark rig: a ring, a check or a cross in the 12 px mark slot at css (102, 12), or an agent icon at (340, 9).
fn mark(s: f32, kind: i32, state: i32, icon_kind: i32) -> Harness<MarkPreview> {
    let h = Harness::new(s, 400, 36, || MarkPreview::new().unwrap());
    h.comp.set_ink(slint::Color::from_rgb_u8(222, 218, 207));
    h.comp.set_ground(slint::Color::from_rgb_u8(23, 23, 23));
    h.comp.set_mark_kind(kind);
    h.comp.set_mark_state(state);
    h.comp.set_icon_kind(icon_kind);
    h
}

#[test]
fn tab_mark_ring_draws_twelve_css_px_at_every_scale() {
    for s in SCALES {
        let f = mark(s, 1, 1, 0).render();
        let (cx, cy) = (108.0 * s, 18.0 * s);
        let region = region_around(&f, cx, cy, 9.0 * s);
        let bg = f.px(region.0, region.1);
        // The track is the ink at 24 percent; the spinner arc sits on the same circle.
        let track = blend(bg, TEXT, 0.24);
        let got = ink_edges(&f, region, &Ink::new(bg, track)).expect("the ring is drawn");
        let want = expected_extent_px(RING_MARK, 12.0, 12.0, 2.0, s);
        assert_size("tab mark ring", s, got, want);
    }
}

#[test]
fn tab_mark_check_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let f = mark(s, 1, 3, 0).render();
        let (cx, cy) = (108.0 * s, 18.0 * s);
        let region = region_around(&f, cx, cy, 9.0 * s);
        let bg = f.px(region.0, region.1);
        let got = ink_edges(&f, region, &Ink::new(bg, TEXT)).expect("the check is drawn");
        let want = expected_extent_px(CHECK, 12.0, 16.0, 2.4 * 12.0 / 16.0, s);
        assert_size("tab mark check", s, got, want);
    }
}

#[test]
fn tab_mark_cross_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let f = mark(s, 1, 4, 0).render();
        let (cx, cy) = (108.0 * s, 18.0 * s);
        let region = region_around(&f, cx, cy, 9.0 * s);
        let bg = f.px(region.0, region.1);
        let got = ink_edges(&f, region, &Ink::new(bg, TEXT)).expect("the cross is drawn");
        let want = expected_extent_px(CROSS, 10.0, 16.0, 2.4 * 10.0 / 16.0, s);
        assert_size("tab mark cross", s, got, want);
    }
}

#[test]
fn agent_prompt_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let f = mark(s, 0, 0, 0).render();
        let (cx, cy) = (349.0 * s, 18.0 * s);
        let region = region_around(&f, cx, cy, 13.0 * s);
        let bg = f.px(region.0, region.1);
        let got = ink_edges(&f, region, &Ink::new(bg, TEXT)).expect("the prompt is drawn");
        let want = expected_extent_px(PROMPT, 18.0, 24.0, 2.2 * 18.0 / 24.0, s);
        assert_size("agent prompt", s, got, want);
    }
}

#[test]
fn agent_cog_draws_at_full_size_at_every_scale() {
    for s in SCALES {
        let f = mark(s, 0, 0, 3).render();
        let (cx, cy) = (349.0 * s, 18.0 * s);
        let region = region_around(&f, cx, cy, 13.0 * s);
        let bg = f.px(region.0, region.1);
        let got = ink_edges(&f, region, &Ink::new(bg, TEXT)).expect("the cog is drawn");
        let want = expected_extent_px(COG, 18.0, 24.0, 1.7 * 18.0 / 24.0, s);
        assert_size("agent cog", s, got, want);
    }
}
