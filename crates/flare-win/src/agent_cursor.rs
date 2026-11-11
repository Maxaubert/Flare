//! The agent's cursor over the web panel: an orange arrow with a white rim and a name tag, a ripple
//! on each press. Flare draws it in a window of its own, never in the page (docs/surfaces/decisions.md):
//! the page cannot see, move or hide it and it is in no screenshot the agent takes.
//!
//! Part A is pure pixels: the arrow as a polygon with analytic anti-aliasing (the taskbar disc's
//! approach), the tag's text as a GDI coverage mask (as `taskbar::text_mask`), composed premultiplied
//! BGRA. Part B is the window: a small layered popup owned by Flare's main window, transparent to the
//! mouse, never activated, with no taskbar button. A trait stands in for it in the tests and `--e2e`.
//!
//! The glow round the page while the agent uses it (Max, 2026-10-27: first a 2 px ring, then a soft glow in
//! from the edges) is drawn the same way, in four more such windows, one per edge band: the browser is a
//! child window Slint cannot draw over, and a layered popup shows above it while every click goes through to
//! the page. Only the bands are drawn, so a 4K page costs a few megabytes, not a page-sized bitmap.

/// The default look's accent orange as (r, g, b), fixed: the cursor is the same in every theme.
const ORANGE: (u8, u8, u8) = (0xfe, 0x8f, 0x34);
/// The tag's text, near black.
const INK: (u8, u8, u8) = (0x1a, 0x1a, 0x1a);
/// The arrow in units of a 24 unit box drawn 22 px wide at 100 %, from its tip (mockup A's path).
const ARROW: [(f64, f64); 4] = [(0.0, 0.0), (15.0, 9.2), (8.4, 10.7), (5.0, 17.0)];
/// Pixels per arrow unit at 100 %.
const UNIT: f64 = 22.0 / 24.0;
/// The white rim, centred on the arrow's edge, in pixels at 100 %.
const RIM_PX: f64 = 1.6;
/// The ripple's disc at full size (scale 1), in pixels across at 100 %; it grows from 0.3 to 1.4 of it.
const RIPPLE_PX: f64 = 34.0;
/// The ripple's opacity when it starts (the mockup's orange at 0.55, faded from 0.9).
const RIPPLE_ALPHA: f64 = 0.5;
/// Where the tag's top left sits from the tip, its height, its side padding and its text, at 100 %.
const TAG_AT: (f64, f64) = (14.0, 14.0);
const TAG_H: f64 = 19.0;
const TAG_PAD: f64 = 7.0;
const TAG_FONT: f64 = 11.0;
/// A name longer than this is cut (the tag stays a tag).
const TAG_CHARS: usize = 24;
/// The glow in from the page's edges: the cursor's orange at this opacity right at an edge, fading to
/// nothing this many pixels in at 100 % (Max, 2026-10-27).
const GLOW_ALPHA: f64 = 0.5;
const GLOW_PX: f64 = 32.0;
/// The largest glow drawn, in pixels each way (a bigger page has none rather than a huge bitmap).
const GLOW_MAX: u32 = 16_384;

/// A rendered cursor: premultiplied BGRA, top-down, `w * h * 4` bytes, with the tip at `tip`.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorBitmap {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
    pub tip: (i32, i32),
}

/// The scale as used for drawing: finite, above 0, at most 4; else 1.
fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale.min(4.0)
    } else {
        1.0
    }
}

/// Signed distance from (x, y) to the polygon's edge: negative inside.
fn polygon_distance(x: f64, y: f64, pts: &[(f64, f64)]) -> f64 {
    let mut best = f64::MAX;
    let mut inside = false;
    for i in 0..pts.len() {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % pts.len()];
        let (ex, ey) = (bx - ax, by - ay);
        let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (dx, dy) = (x - ax - ex * t, y - ay - ey * t);
        best = best.min((dx * dx + dy * dy).sqrt());
        // Even-odd crossing test on the edge (a, b).
        if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * ex {
            inside = !inside;
        }
    }
    if inside { -best } else { best }
}

/// The arrow's coverage at `scale`: the whole shape with its rim (`outer`) and the orange inside the
/// rim (`fill`), each 0..1, on a `w * h` box whose tip is at `tip`.
pub struct ArrowCoverage {
    pub w: u32,
    pub h: u32,
    pub tip: (u32, u32),
    pub outer: Vec<f32>,
    pub fill: Vec<f32>,
}

/// Analytic anti-aliasing, as the taskbar disc: coverage = clamp(edge - distance + 0.5, 0, 1), with the
/// rim's outer edge `RIM_PX / 2` outside the path and the fill's the same inside.
pub fn arrow_coverage(scale: f64) -> ArrowCoverage {
    let s = clean_scale(scale);
    let k = UNIT * s;
    let half = RIM_PX * s / 2.0;
    let pad = half.ceil() as u32 + 1;
    let pts: Vec<(f64, f64)> = ARROW
        .iter()
        .map(|(x, y)| (x * k + f64::from(pad), y * k + f64::from(pad)))
        .collect();
    let max_x = ARROW.iter().map(|p| p.0).fold(0.0, f64::max) * k;
    let max_y = ARROW.iter().map(|p| p.1).fold(0.0, f64::max) * k;
    let w = max_x.ceil() as u32 + 2 * pad;
    let h = max_y.ceil() as u32 + 2 * pad;
    let mut outer = Vec::with_capacity((w * h) as usize);
    let mut fill = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let d = polygon_distance(f64::from(x) + 0.5, f64::from(y) + 0.5, &pts);
            outer.push((half - d + 0.5).clamp(0.0, 1.0) as f32);
            fill.push((-half - d + 0.5).clamp(0.0, 1.0) as f32);
        }
    }
    ArrowCoverage {
        w,
        h,
        tip: (pad, pad),
        outer,
        fill,
    }
}

/// Coverage of a pill (a rectangle with half-height round ends) at the pixel's centre.
fn pill_coverage(x: f64, y: f64, w: f64, h: f64) -> f64 {
    let r = h / 2.0;
    let cx = x.clamp(r, (w - r).max(r));
    let d = ((x - cx).powi(2) + (y - r).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// The tag's words: the agent's name, cut to [`TAG_CHARS`].
fn tag_text(label: &str) -> String {
    label.trim().chars().take(TAG_CHARS).collect()
}

/// Premultiplied `colour` at `a` (0..1) over premultiplied `under` (BGRA).
fn over(under: [f32; 4], colour: (u8, u8, u8), a: f32) -> [f32; 4] {
    let k = 1.0 - a;
    [
        f32::from(colour.2) * a + under[0] * k,
        f32::from(colour.1) * a + under[1] * k,
        f32::from(colour.0) * a + under[2] * k,
        255.0 * a + under[3] * k,
    ]
}

/// The arrow and its tag drawn once for a scale and a name; [`CursorArt::frame`] adds the ripples.
pub struct CursorArt {
    w: u32,
    h: u32,
    tip: (i32, i32),
    scale: f64,
    /// Premultiplied BGRA, 0..255 as floats.
    base: Vec<[f32; 4]>,
}

impl CursorArt {
    pub fn new(scale: f64, label: &str) -> Self {
        let s = clean_scale(scale);
        // Room up and left of the tip for the ripple at its largest.
        let m = (RIPPLE_PX * 1.4 / 2.0 * s).ceil() as i32 + 1;
        let arrow = arrow_coverage(s);
        let text = tag_text(label);
        let tag_h = (TAG_H * s).round().max(1.0) as u32;
        let (text_w, mask) = label_mask((TAG_FONT * s).round().max(1.0) as u32, tag_h, &text);
        let pad = (TAG_PAD * s).round() as u32;
        let tag_w = if text.is_empty() { 0 } else { text_w + 2 * pad };
        let tag_x = m + (TAG_AT.0 * s).round() as i32;
        let tag_y = m + (TAG_AT.1 * s).round() as i32;
        let ax = m - arrow.tip.0 as i32;
        let ay = m - arrow.tip.1 as i32;
        let w = (2 * m + 1)
            .max(tag_x + tag_w as i32 + 1)
            .max(ax + arrow.w as i32) as u32;
        let h = (2 * m + 1)
            .max(tag_y + tag_h as i32 + 1)
            .max(ay + arrow.h as i32) as u32;
        let mut base = vec![[0.0f32; 4]; (w * h) as usize];
        // The tag first (under the arrow): an orange pill with the name in ink.
        if tag_w > 0 {
            for ty in 0..tag_h {
                for tx in 0..tag_w {
                    let a = pill_coverage(
                        f64::from(tx) + 0.5,
                        f64::from(ty) + 0.5,
                        f64::from(tag_w),
                        f64::from(tag_h),
                    ) as f32;
                    if a <= 0.0 {
                        continue;
                    }
                    let i = ((tag_y as u32 + ty) * w + tag_x as u32 + tx) as usize;
                    let mut p = over(base[i], ORANGE, a);
                    if tx >= pad && tx - pad < text_w {
                        let ink = f32::from(mask[(ty * text_w + tx - pad) as usize]) / 255.0;
                        p = over(p, INK, ink * a);
                    }
                    base[i] = p;
                }
            }
        }
        // The arrow: white where the rim is, orange inside it.
        for y in 0..arrow.h {
            for x in 0..arrow.w {
                let j = (y * arrow.w + x) as usize;
                let (o, f) = (arrow.outer[j], arrow.fill[j]);
                if o <= 0.0 {
                    continue;
                }
                let i = ((ay as u32 + y) * w + ax as u32 + x) as usize;
                // The rim's white over what is below, then the orange over the rim.
                let p = over(base[i], (255, 255, 255), o);
                base[i] = over(p, ORANGE, f);
            }
        }
        Self {
            w,
            h,
            tip: (m, m),
            scale: s,
            base,
        }
    }

    /// The bitmap with a ripple for each progress in `ripples` (0..1) under the arrow.
    pub fn frame(&self, ripples: &[f32]) -> CursorBitmap {
        let (cx, cy) = (f64::from(self.tip.0), f64::from(self.tip.1));
        let discs: Vec<(f64, f64)> = ripples
            .iter()
            .map(|t| {
                let t = f64::from(t.clamp(0.0, 1.0));
                let ease = 1.0 - (1.0 - t).powi(3);
                let r = RIPPLE_PX / 2.0 * self.scale * (0.3 + 1.1 * ease);
                (r, RIPPLE_ALPHA * (1.0 - t))
            })
            .collect();
        let mut bgra = Vec::with_capacity(self.base.len() * 4);
        for (i, p) in self.base.iter().enumerate() {
            let mut under = [0.0f32; 4];
            if !discs.is_empty() {
                let x = (i as u32 % self.w) as f64 + 0.5 - cx;
                let y = (i as u32 / self.w) as f64 + 0.5 - cy;
                let d = (x * x + y * y).sqrt();
                for (r, a) in &discs {
                    let c = (r - d + 0.5).clamp(0.0, 1.0) * a;
                    under = over(under, ORANGE, c as f32);
                }
            }
            // The art over the ripples.
            let k = 1.0 - p[3] / 255.0;
            for ch in 0..4 {
                bgra.push((p[ch] + under[ch] * k).round().clamp(0.0, 255.0) as u8);
            }
        }
        CursorBitmap {
            w: self.w,
            h: self.h,
            bgra,
            tip: self.tip,
        }
    }
}

/// The cursor at `scale` with its tag naming `label` and a ripple for each progress in `ripples`.
pub fn compose_cursor(scale: f64, label: &str, ripples: &[f32]) -> CursorBitmap {
    CursorArt::new(scale, label).frame(ripples)
}

/// How strong the glow is `d` pixels in from one edge, for a glow `r` pixels deep: 1 at the edge, eased
/// (quadratic) to 0 at `r`, so it fades with no band and no hard inner line.
fn glow_falloff(d: f64, r: f64) -> f64 {
    if d >= r {
        0.0
    } else {
        let t = 1.0 - d / r;
        t * t
    }
}

/// The glow along one axis of `n` pixels: both its edges, combined as light is (`1 - (1 - a)(1 - b)`).
fn glow_ramp(n: u32, r: f64) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let a = glow_falloff(f64::from(i) + 0.5, r);
            let b = glow_falloff(f64::from(n - i) - 0.5, r);
            1.0 - (1.0 - a) * (1.0 - b)
        })
        .collect()
}

/// The glow of a page: its size and its edge bands, each with where its top left sits in the page.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeGlow {
    pub w: u32,
    pub h: u32,
    pub bands: Vec<((i32, i32), CursorBitmap)>,
}

/// The glow's pixels in the page's rectangle `x0, y0, w, h`, from the two axes' ramps.
fn glow_region(gx: &[f64], gy: &[f64], (x0, y0, w, h): (u32, u32, u32, u32)) -> CursorBitmap {
    // Each opacity's premultiplied pixel, looked up rather than worked out per pixel.
    let lut: Vec<[u8; 4]> = (0..=255u32)
        .map(|a| {
            let k = f64::from(a) / 255.0;
            [
                (f64::from(ORANGE.2) * k).round() as u8,
                (f64::from(ORANGE.1) * k).round() as u8,
                (f64::from(ORANGE.0) * k).round() as u8,
                a as u8,
            ]
        })
        .collect();
    let peak = 255.0 * GLOW_ALPHA;
    let gx = &gx[x0 as usize..(x0 + w) as usize];
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let mut last: Option<(f64, usize)> = None;
    for (i, &y) in gy[y0 as usize..(y0 + h) as usize].iter().enumerate() {
        let at = i * (w * 4) as usize;
        // A row like the one above (the side bands' rows all are) is copied, not worked out again.
        if let Some((ly, from)) = last
            && ly == y
        {
            bgra.copy_within(from..from + (w * 4) as usize, at);
            continue;
        }
        let row = &mut bgra[at..at + (w * 4) as usize];
        for (p, &x) in row.as_chunks_mut::<4>().0.iter_mut().zip(gx) {
            *p = lut[(peak * (1.0 - (1.0 - x) * (1.0 - y))).round() as usize];
        }
        last = Some((y, at));
    }
    CursorBitmap {
        w,
        h,
        bgra,
        tip: (0, 0),
    }
}

/// The glow for a page of `w * h` physical pixels at `scale`: the cursor's orange at [`GLOW_ALPHA`] right at
/// each edge, fading over [`GLOW_PX`] (at 100 %) to clear. The edges combine as light does, so a corner is
/// no stronger than an edge and blends with no seam or darker square. It comes as up to four bands (top and
/// bottom the page's width, the sides between them): everything outside them is clear, so it is never
/// drawn. None for an empty or too large page.
pub fn edge_glow(w: u32, h: u32, scale: f64) -> EdgeGlow {
    if w == 0 || h == 0 || w > GLOW_MAX || h > GLOW_MAX {
        return EdgeGlow {
            w: 0,
            h: 0,
            bands: Vec::new(),
        };
    }
    let r = (GLOW_PX * clean_scale(scale)).round().max(1.0);
    let ru = r as u32;
    let (gx, gy) = (glow_ramp(w, r), glow_ramp(h, r));
    let top = ru.min(h);
    let low = h.saturating_sub(ru).max(top);
    let left = ru.min(w);
    let right = w.saturating_sub(ru).max(left);
    let rects = [
        (0, 0, w, top),
        (0, low, w, h - low),
        (0, top, left, low - top),
        (right, top, w - right, low - top),
    ];
    let bands = rects
        .into_iter()
        .filter(|&(_, _, bw, bh)| bw > 0 && bh > 0)
        .map(|rect| ((rect.0 as i32, rect.1 as i32), glow_region(&gx, &gy, rect)))
        .collect();
    EdgeGlow { w, h, bands }
}

/// The whole page's glow as one bitmap (the tests check the bands against it).
#[cfg(test)]
fn glow_bitmap(w: u32, h: u32, scale: f64) -> CursorBitmap {
    let r = (GLOW_PX * clean_scale(scale)).round().max(1.0);
    glow_region(&glow_ramp(w, r), &glow_ramp(h, r), (0, 0, w, h))
}

// ---------------------------------------------------------------- Arrows over the page (2026-11-10)

/// The person's arrow: a clear blue, fixed in every theme, so it is never taken for the agent's orange.
pub const BLUE: (u8, u8, u8) = (0x2f, 0x7c, 0xf6);
/// The arrow's shaft, its head's length and width, at 100 %.
const SHAFT_PX: f64 = 4.0;
const HEAD_LEN_PX: f64 = 16.0;
const HEAD_W_PX: f64 = 15.0;
/// How long the agent's arrow is from its tail to the head it points with, at 100 %.
pub const POINT_TAIL_PX: f64 = 64.0;
/// The longest label an arrow carries (characters); `ask_agent` cleans it before it comes here.
pub const POINT_LABEL_CHARS: usize = 48;
/// The longest arrow drawn, in pixels each way (a bigger one is cut to it rather than a huge bitmap).
const ARROW_MAX: f64 = 8_192.0;

/// Whose arrow it is: the agent's is orange and may carry a label, the person's is blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowInk {
    Agent,
    Person,
}

impl ArrowInk {
    fn colour(self) -> (u8, u8, u8) {
        match self {
            ArrowInk::Agent => ORANGE,
            ArrowInk::Person => BLUE,
        }
    }
}

/// Distance from (x, y) to the segment a-b.
fn segment_distance(x: f64, y: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (((x - a.0) * ex + (y - a.1) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (x - a.0 - ex * t, y - a.1 - ey * t);
    (dx * dx + dy * dy).sqrt()
}

/// Where the tail of the agent's arrow goes for a head at `head` inside the page `rect` (x, y, w, h, screen or
/// client pixels alike), `len` pixels away: down and right of the head as a pointer is, turned to whichever
/// diagonal keeps the tail inside the page when the head is near an edge. The answer is from the head.
pub fn point_tail(head: (i32, i32), rect: (i32, i32, i32, i32), len: f64) -> (f64, f64) {
    let d = len / std::f64::consts::SQRT_2;
    let (x0, y0, x1, y1) = (
        f64::from(rect.0),
        f64::from(rect.1),
        f64::from(rect.0 + rect.2),
        f64::from(rect.1 + rect.3),
    );
    let inside = |dx: f64, dy: f64| {
        let (x, y) = (f64::from(head.0) + dx, f64::from(head.1) + dy);
        x >= x0 && x < x1 && y >= y0 && y < y1
    };
    [(d, d), (-d, d), (d, -d), (-d, -d)]
        .into_iter()
        .find(|(dx, dy)| inside(*dx, *dy))
        .unwrap_or((d, d))
}

/// An arrow whose head is at the bitmap's `tip` and whose tail is `tail` pixels away from it (any direction), with
/// a white rim like the cursor's, at `scale`. A `label` (the agent's) sits on an orange pill past the tail. An
/// arrow shorter than its head is drawn at its head's length in the same direction (a click with no drag points
/// down and to the right of where it was made, from up and left). Only the pixels near the shape are worked out,
/// so a long arrow across a large page costs its length, not its box.
pub fn compose_arrow(tail: (f64, f64), scale: f64, ink: ArrowInk, label: &str) -> CursorBitmap {
    let s = clean_scale(scale);
    let half = RIM_PX * s / 2.0;
    let head_len = HEAD_LEN_PX * s;
    let head_w = HEAD_W_PX * s;
    let shaft = SHAFT_PX * s / 2.0;
    let mut v = (
        tail.0.clamp(-ARROW_MAX, ARROW_MAX),
        tail.1.clamp(-ARROW_MAX, ARROW_MAX),
    );
    if !(v.0.is_finite() && v.1.is_finite()) {
        v = (0.0, 0.0);
    }
    let min_len = head_len * 1.5;
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len < min_len {
        let (ux, uy) = if len > 0.0 {
            (v.0 / len, v.1 / len)
        } else {
            (
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            )
        };
        v = (ux * min_len, uy * min_len);
    }
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    // From the tail towards the head, and across it.
    let (ux, uy) = (-v.0 / len, -v.1 / len);
    let (nx, ny) = (-uy, ux);
    // The label's pill, past the tail on the side away from the head.
    let text: String = label.trim().chars().take(POINT_LABEL_CHARS).collect();
    let tag_h = (TAG_H * s).round().max(1.0) as u32;
    let (text_w, mask) = if matches!(ink, ArrowInk::Agent) && !text.is_empty() {
        label_mask((TAG_FONT * s).round().max(1.0) as u32, tag_h, &text)
    } else {
        (0, Vec::new())
    };
    let pad = (TAG_PAD * s).round() as u32;
    let tag_w = if text_w == 0 { 0 } else { text_w + 2 * pad };
    let gap = (4.0 * s).round();
    // In head-relative pixels: the pill's top left.
    let tag_at = (
        if v.0 >= 0.0 {
            v.0 + gap
        } else {
            v.0 - gap - f64::from(tag_w)
        },
        if v.1 >= 0.0 {
            v.1 + gap
        } else {
            v.1 - gap - f64::from(tag_h)
        },
    );
    // The box, in head-relative pixels, padded for the rim and the head's width.
    let m = head_w + half + 2.0;
    let (mut bx0, mut by0) = (v.0.min(0.0) - m, v.1.min(0.0) - m);
    let (mut bx1, mut by1) = (v.0.max(0.0) + m, v.1.max(0.0) + m);
    if tag_w > 0 {
        bx0 = bx0.min(tag_at.0 - 1.0);
        by0 = by0.min(tag_at.1 - 1.0);
        bx1 = bx1.max(tag_at.0 + f64::from(tag_w) + 1.0);
        by1 = by1.max(tag_at.1 + f64::from(tag_h) + 1.0);
    }
    let (ox, oy) = (bx0.floor(), by0.floor());
    let w = (bx1 - ox).ceil().max(1.0) as u32;
    let h = (by1 - oy).ceil().max(1.0) as u32;
    let head = (-ox, -oy);
    let tail_pt = (v.0 - ox, v.1 - oy);
    let base = (head.0 - ux * head_len, head.1 - uy * head_len);
    let tri = [
        head,
        (base.0 + nx * head_w / 2.0, base.1 + ny * head_w / 2.0),
        (base.0 - nx * head_w / 2.0, base.1 - ny * head_w / 2.0),
    ];
    // The shaft runs into the head a little, so the two meet with no seam.
    let shaft_end = (base.0 + ux * 2.0 * s, base.1 + uy * 2.0 * s);
    let colour = ink.colour();
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let paint = |px: i64, py: i64, bgra: &mut [u8]| {
        if px < 0 || py < 0 || px >= i64::from(w) || py >= i64::from(h) {
            return;
        }
        let (cx, cy) = (px as f64 + 0.5, py as f64 + 0.5);
        let d = polygon_distance(cx, cy, &tri)
            .min(segment_distance(cx, cy, tail_pt, shaft_end) - shaft);
        let o = (half - d + 0.5).clamp(0.0, 1.0) as f32;
        if o <= 0.0 {
            return;
        }
        let f = (-half - d + 0.5).clamp(0.0, 1.0) as f32;
        let p = over(over([0.0; 4], (255, 255, 255), o), colour, f);
        let i = ((py as u32 * w + px as u32) * 4) as usize;
        for ch in 0..4 {
            bgra[i + ch] = p[ch].round().clamp(0.0, 255.0) as u8;
        }
    };
    // Along the shaft, a square round each step; then the head's box. Every pixel is worked out from the whole
    // shape, so one reached twice comes out the same.
    let reach = (shaft + half + 2.0).ceil() as i64;
    let steps = len.ceil() as i64;
    for k in 0..=steps {
        let t = k as f64 / steps.max(1) as f64;
        let (cx, cy) = (
            tail_pt.0 + (shaft_end.0 - tail_pt.0) * t,
            tail_pt.1 + (shaft_end.1 - tail_pt.1) * t,
        );
        for py in (cy.floor() as i64 - reach)..=(cy.floor() as i64 + reach) {
            for px in (cx.floor() as i64 - reach)..=(cx.floor() as i64 + reach) {
                paint(px, py, &mut bgra);
            }
        }
    }
    let hx0 = tri.iter().map(|p| p.0).fold(f64::MAX, f64::min) - half - 2.0;
    let hy0 = tri.iter().map(|p| p.1).fold(f64::MAX, f64::min) - half - 2.0;
    let hx1 = tri.iter().map(|p| p.0).fold(f64::MIN, f64::max) + half + 2.0;
    let hy1 = tri.iter().map(|p| p.1).fold(f64::MIN, f64::max) + half + 2.0;
    for py in hy0.floor() as i64..=hy1.ceil() as i64 {
        for px in hx0.floor() as i64..=hx1.ceil() as i64 {
            paint(px, py, &mut bgra);
        }
    }
    // The label's pill goes under the arrow: each of its pixels once.
    if tag_w > 0 {
        let (tx0, ty0) = (
            (tag_at.0 - ox).round() as u32,
            (tag_at.1 - oy).round() as u32,
        );
        for ty in 0..tag_h {
            for tx in 0..tag_w {
                let a = pill_coverage(
                    f64::from(tx) + 0.5,
                    f64::from(ty) + 0.5,
                    f64::from(tag_w),
                    f64::from(tag_h),
                ) as f32;
                if a <= 0.0 || tx0 + tx >= w || ty0 + ty >= h {
                    continue;
                }
                let mut p = over([0.0; 4], ORANGE, a);
                if tx >= pad && tx - pad < text_w {
                    let ink_a = f32::from(mask[(ty * text_w + tx - pad) as usize]) / 255.0;
                    p = over(p, INK, ink_a * a);
                }
                let i = (((ty0 + ty) * w + tx0 + tx) * 4) as usize;
                let k = 1.0 - f32::from(bgra[i + 3]) / 255.0;
                for ch in 0..4 {
                    bgra[i + ch] = (f32::from(bgra[i + ch]) + p[ch] * k)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    CursorBitmap {
        w,
        h,
        bgra,
        tip: (head.0.round() as i32, head.1.round() as i32),
    }
}

/// Lays `bmp` (premultiplied BGRA) over a straight RGBA picture of `w * h` with its tip at (x, y): an arrow drawn into
/// a screenshot, where it is on the screen. Whatever falls outside the picture is left out.
pub fn blend_onto_rgba(rgba: &mut [u8], w: u32, h: u32, at: (i32, i32), bmp: &CursorBitmap) {
    if rgba.len() != (w as usize) * (h as usize) * 4
        || bmp.bgra.len() != (bmp.w * bmp.h * 4) as usize
    {
        return;
    }
    let (x0, y0) = (at.0 - bmp.tip.0, at.1 - bmp.tip.1);
    for by in 0..bmp.h {
        let y = y0 + by as i32;
        if y < 0 || y >= h as i32 {
            continue;
        }
        for bx in 0..bmp.w {
            let x = x0 + bx as i32;
            if x < 0 || x >= w as i32 {
                continue;
            }
            let s = ((by * bmp.w + bx) * 4) as usize;
            let a = u32::from(bmp.bgra[s + 3]);
            if a == 0 {
                continue;
            }
            let d = ((y as u32 * w + x as u32) * 4) as usize;
            let k = 255 - a;
            // Premultiplied source over the picture (its alpha kept: a screenshot is opaque).
            for (ch, src) in [(0, 2), (1, 1), (2, 0)] {
                let under = u32::from(rgba[d + ch]);
                rgba[d + ch] =
                    ((u32::from(bmp.bgra[s + src]) * 255 + under * k + 127) / 255).min(255) as u8;
            }
            rgba[d + 3] = (a + u32::from(rgba[d + 3]) * k / 255).min(255) as u8;
        }
    }
}

// ---------------------------------------------------------------- Part B: GDI and the window

#[cfg(windows)]
mod win {
    use super::CursorBitmap;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
        BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
        DEFAULT_CHARSET, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
        DeleteObject, DrawTextW, GetTextExtentPoint32W, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HTCLIENT, HTTRANSPARENT, IDC_CROSS,
        LoadCursorW, MA_NOACTIVATE, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SetCursor,
        ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
        WM_MOUSEMOVE, WM_NCHITTEST, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    const CLASS: &str = "FlareAgentCursor";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn dib_info(w: u32, h: u32) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        }
    }

    /// The tag's text, white on black, read back as 0..255 coverage: (width, `width * h` bytes).
    pub fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
        if text.is_empty() || h == 0 {
            return (0, Vec::new());
        }
        let face = wide("Segoe UI");
        let w16: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: a memory DC and a font made here and released below; `face` outlives the call.
        let (hdc, font) = unsafe {
            let hdc = CreateCompatibleDC(null_mut());
            let font = CreateFontW(
                -(font_px as i32),
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                0,
                face.as_ptr(),
            );
            (hdc, font)
        };
        let mut ext = SIZE { cx: 0, cy: 0 };
        // SAFETY: the font is ours and selected into our DC; `w16` and `ext` live across the call.
        let old_font = unsafe {
            let old = SelectObject(hdc, font);
            GetTextExtentPoint32W(hdc, w16.as_ptr(), w16.len() as i32, &mut ext);
            old
        };
        let w = (ext.cx.max(0) as u32 + 1).min(4096);
        let n = (w * h) as usize;
        let info = dib_info(w, h);
        let mut bits: *mut c_void = null_mut();
        // SAFETY: a 32-bit DIB section on our DC; GDI fills `bits`.
        let bmp = unsafe { CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        let mut out = vec![0u8; n];
        if !bmp.is_null() && !bits.is_null() {
            let mut w16 = w16.clone();
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            // SAFETY: our bitmap into our DC; `bits` holds n * 4 writable bytes; `w16` and `rc` live
            // across the call.
            unsafe {
                let old_bmp = SelectObject(hdc, bmp);
                std::ptr::write_bytes(bits as *mut u8, 0, n * 4);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, 0x00FF_FFFF);
                DrawTextW(
                    hdc,
                    w16.as_mut_ptr(),
                    w16.len() as i32,
                    &mut rc,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
                // BGRA: the red channel is byte 2.
                for (o, p) in out.iter_mut().zip(src.as_chunks::<4>().0.iter()) {
                    *o = p[2];
                }
                SelectObject(hdc, old_bmp);
            }
        }
        // SAFETY: release everything made here, after the last read of `bits`.
        unsafe {
            SelectObject(hdc, old_font);
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            DeleteObject(font);
            DeleteDC(hdc);
        }
        (w, out)
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            // Never the mouse's, never activated.
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            // SAFETY: the default handling of the window's own message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn register() -> bool {
        static ONCE: Once = Once::new();
        static OK: AtomicBool = AtomicBool::new(false);
        ONCE.call_once(|| {
            // SAFETY: the class name is leaked so it outlives the class; the procedure is ours.
            let ok = unsafe {
                let name: &'static [u16] = Box::leak(wide(CLASS).into_boxed_slice());
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(null()),
                    hIcon: null_mut(),
                    hCursor: null_mut(),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: name.as_ptr(),
                };
                RegisterClassW(&wc) != 0
            };
            OK.store(ok, Ordering::SeqCst);
        });
        OK.load(Ordering::SeqCst)
    }

    /// The real window: made the first time the cursor is shown, destroyed with this value.
    pub struct CursorWindow {
        owner: isize,
        hwnd: isize,
        visible: bool,
    }

    impl CursorWindow {
        pub fn new(owner: isize) -> Self {
            Self {
                owner,
                hwnd: 0,
                visible: false,
            }
        }

        fn window(&mut self) -> Option<HWND> {
            if self.hwnd == 0 && self.owner != 0 && register() {
                let class = wide(CLASS);
                // SAFETY: a popup owned by Flare's own window, of the class registered above.
                let h = unsafe {
                    CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                        class.as_ptr(),
                        class.as_ptr(),
                        WS_POPUP,
                        0,
                        0,
                        1,
                        1,
                        self.owner as HWND,
                        null_mut(),
                        GetModuleHandleW(null()),
                        null(),
                    )
                };
                if h.is_null() {
                    log::warn!("agent cursor: the window was not made");
                    // Not asked again: the cursor is a nicety, not worth a retry per frame.
                    self.owner = 0;
                } else {
                    self.hwnd = h as isize;
                }
            }
            (self.hwnd != 0).then_some(self.hwnd as HWND)
        }
    }

    /// Gives a layered window of ours a picture of `w * h` premultiplied BGRA pixels, which `fill` writes, with its top
    /// left at (x, y) on the screen and `alpha` over all. True when Windows took it.
    fn layered_update(
        hwnd: HWND,
        (x, y): (i32, i32),
        (w, h): (u32, u32),
        alpha: u8,
        fill: impl FnOnce(&mut [u8]),
    ) -> bool {
        if w == 0 || h == 0 {
            return false;
        }
        let info = dib_info(w, h);
        let mut bits: *mut c_void = null_mut();
        // SAFETY: a memory DC and a DIB section the size of the picture, released below.
        let (hdc, dib) = unsafe {
            let hdc = CreateCompatibleDC(null_mut());
            let dib = CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            (hdc, dib)
        };
        let mut ok = false;
        if !dib.is_null() && !bits.is_null() {
            let dst = POINT { x, y };
            let size = SIZE {
                cx: w as i32,
                cy: h as i32,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // SAFETY: `bits` holds w * h * 4 writable bytes, the DIB section's own; the window is ours and layered;
            // every pointer lives across the call.
            unsafe {
                fill(std::slice::from_raw_parts_mut(
                    bits as *mut u8,
                    (w as usize) * (h as usize) * 4,
                ));
                let old = SelectObject(hdc, dib);
                ok = UpdateLayeredWindow(
                    hwnd,
                    null_mut(),
                    &dst,
                    &size,
                    hdc,
                    &src,
                    0,
                    &blend,
                    ULW_ALPHA,
                ) != 0;
                SelectObject(hdc, old);
            }
        }
        // SAFETY: made above.
        unsafe {
            if !dib.is_null() {
                DeleteObject(dib);
            }
            DeleteDC(hdc);
        }
        ok
    }

    impl super::CursorShell for CursorWindow {
        fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
            if bmp.w == 0 || bmp.h == 0 || bmp.bgra.len() != (bmp.w * bmp.h * 4) as usize {
                return false;
            }
            let Some(hwnd) = self.window() else {
                return false;
            };
            let ok = layered_update(
                hwnd,
                (x - bmp.tip.0, y - bmp.tip.1),
                (bmp.w, bmp.h),
                alpha,
                |bits| bits.copy_from_slice(&bmp.bgra),
            );
            if ok && !self.visible {
                // SAFETY: our own window; shown without taking the focus.
                unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                self.visible = true;
            }
            ok
        }

        fn set_alpha(&mut self, alpha: u8) -> bool {
            if !self.visible || self.hwnd == 0 {
                return false;
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // SAFETY: our own layered window; with no source DC it keeps its picture and place and takes
            // only the new blend, which lives across the call.
            unsafe {
                UpdateLayeredWindow(
                    self.hwnd as HWND,
                    null_mut(),
                    null(),
                    null(),
                    null_mut(),
                    null(),
                    0,
                    &blend,
                    ULW_ALPHA,
                ) != 0
            }
        }

        fn hide(&mut self) {
            if self.visible && self.hwnd != 0 {
                // SAFETY: our own window.
                unsafe { ShowWindow(self.hwnd as HWND, SW_HIDE) };
            }
            self.visible = false;
        }
    }

    impl Drop for CursorWindow {
        fn drop(&mut self) {
            if self.hwnd != 0 {
                // SAFETY: our own window, on the thread that made it.
                unsafe { DestroyWindow(self.hwnd as HWND) };
                self.hwnd = 0;
            }
        }
    }

    // ---- the pad the person draws an arrow on ----------------------------------------------------------------

    const PAD_CLASS: &str = "FlareArrowPad";

    /// What the pad's window heard: presses, moves and releases (the probe reads them).
    static PAD_DOWN: AtomicU32 = AtomicU32::new(0);
    static PAD_MOVE: AtomicU32 = AtomicU32::new(0);
    static PAD_UP: AtomicU32 = AtomicU32::new(0);

    /// (presses, moves, releases) the pad's windows have had since the process began.
    pub fn pad_heard() -> (u32, u32, u32) {
        (
            PAD_DOWN.load(Ordering::SeqCst),
            PAD_MOVE.load(Ordering::SeqCst),
            PAD_UP.load(Ordering::SeqCst),
        )
    }

    unsafe extern "system" fn pad_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            // The mouse's everywhere on the pad, but never activated: the page keeps the keyboard.
            WM_NCHITTEST => HTCLIENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_SETCURSOR => {
                // SAFETY: a system cursor, set while the pointer is over our own window.
                unsafe { SetCursor(LoadCursorW(null_mut(), IDC_CROSS)) };
                1
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                PAD_DOWN.fetch_add(1, Ordering::SeqCst);
                // SAFETY: our own window takes the mouse until the button is let go (no keyboard focus moves).
                unsafe { SetCapture(hwnd) };
                0
            }
            WM_MOUSEMOVE => {
                PAD_MOVE.fetch_add(1, Ordering::SeqCst);
                0
            }
            WM_LBUTTONUP | WM_RBUTTONUP => {
                PAD_UP.fetch_add(1, Ordering::SeqCst);
                // SAFETY: lets go of the capture taken above.
                unsafe { ReleaseCapture() };
                0
            }
            // SAFETY: the default handling of the window's own message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn register_pad() -> bool {
        static ONCE: Once = Once::new();
        static OK: AtomicBool = AtomicBool::new(false);
        ONCE.call_once(|| {
            // SAFETY: the class name is leaked so it outlives the class; the procedure is ours.
            let ok = unsafe {
                let name: &'static [u16] = Box::leak(wide(PAD_CLASS).into_boxed_slice());
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(pad_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(null()),
                    hIcon: null_mut(),
                    hCursor: null_mut(),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: name.as_ptr(),
                };
                RegisterClassW(&wc) != 0
            };
            OK.store(ok, Ordering::SeqCst);
        });
        OK.load(Ordering::SeqCst)
    }

    /// The pad over the page while the person draws an arrow: a layered popup owned by Flare's window, every pixel at
    /// alpha 1 (all but clear, and so the mouse's), never activated, so a press on it reaches neither the page nor the
    /// keyboard focus. Made the first time it is shown, destroyed with this value.
    pub struct PadWindow {
        owner: isize,
        hwnd: isize,
        /// Where it covers now, while shown.
        at: Option<(i32, i32, u32, u32)>,
    }

    impl PadWindow {
        pub fn new(owner: isize) -> Self {
            Self {
                owner,
                hwnd: 0,
                at: None,
            }
        }

        /// The window, for the probe.
        pub fn hwnd(&self) -> isize {
            self.hwnd
        }

        fn window(&mut self) -> Option<HWND> {
            if self.hwnd == 0 && self.owner != 0 && register_pad() {
                let class = wide(PAD_CLASS);
                // SAFETY: a popup owned by Flare's own window, of the class registered above.
                let h = unsafe {
                    CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                        class.as_ptr(),
                        class.as_ptr(),
                        WS_POPUP,
                        0,
                        0,
                        1,
                        1,
                        self.owner as HWND,
                        null_mut(),
                        GetModuleHandleW(null()),
                        null(),
                    )
                };
                if h.is_null() {
                    log::warn!("arrow pad: the window was not made");
                    self.owner = 0;
                } else {
                    self.hwnd = h as isize;
                }
            }
            (self.hwnd != 0).then_some(self.hwnd as HWND)
        }
    }

    impl super::PadShell for PadWindow {
        fn show(&mut self, x: i32, y: i32, w: u32, h: u32) -> bool {
            if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
                return false;
            }
            if self.at == Some((x, y, w, h)) {
                return true;
            }
            let Some(hwnd) = self.window() else {
                return false;
            };
            let ok = layered_update(hwnd, (x, y), (w, h), 255, |bits| {
                for p in bits.as_chunks_mut::<4>().0 {
                    *p = [0, 0, 0, 1];
                }
            });
            if ok {
                if self.at.is_none() {
                    // SAFETY: our own window; shown without taking the focus.
                    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                }
                self.at = Some((x, y, w, h));
            }
            ok
        }

        fn hide(&mut self) {
            if self.at.take().is_some() && self.hwnd != 0 {
                // SAFETY: our own window; a capture it holds goes with it.
                unsafe {
                    ReleaseCapture();
                    ShowWindow(self.hwnd as HWND, SW_HIDE);
                }
            }
        }
    }

    impl Drop for PadWindow {
        fn drop(&mut self) {
            if self.hwnd != 0 {
                // SAFETY: our own window, on the thread that made it.
                unsafe { DestroyWindow(self.hwnd as HWND) };
                self.hwnd = 0;
            }
        }
    }
}

#[cfg(windows)]
pub use win::{CursorWindow, PadWindow, pad_heard};

/// The agent's arrow (and the person's kept one): a window of its own like the cursor's, transparent to the mouse.
#[cfg(windows)]
pub struct ArrowWindow(CursorWindow);

#[cfg(windows)]
impl ArrowWindow {
    pub fn new(owner: isize) -> Self {
        Self(CursorWindow::new(owner))
    }
}

#[cfg(windows)]
impl CursorShell for ArrowWindow {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        self.0.show_at(x, y, bmp, alpha)
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        self.0.set_alpha(alpha)
    }

    fn hide(&mut self) {
        self.0.hide();
    }
}

/// The glow's windows: one per edge band, each a [`CursorWindow`].
#[cfg(windows)]
pub struct GlowWindow {
    parts: Vec<CursorWindow>,
    /// How many of `parts` the glow last shown uses (a small page has fewer than four bands).
    used: usize,
}

#[cfg(windows)]
impl GlowWindow {
    pub fn new(owner: isize) -> Self {
        Self {
            parts: (0..4).map(|_| CursorWindow::new(owner)).collect(),
            used: 0,
        }
    }
}

#[cfg(windows)]
impl GlowShell for GlowWindow {
    fn show_at(&mut self, x: i32, y: i32, glow: &EdgeGlow, alpha: u8) -> bool {
        if glow.bands.is_empty() {
            return false;
        }
        self.used = glow.bands.len().min(self.parts.len());
        let mut ok = true;
        for (i, part) in self.parts.iter_mut().enumerate() {
            match glow.bands.get(i) {
                Some(((bx, by), bmp)) => ok &= part.show_at(x + bx, y + by, bmp, alpha),
                None => part.hide(),
            }
        }
        ok
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        // Every part shown takes the new blend; one that cannot has the caller show the glow again whole. The
        // parts a small page leaves hidden are not asked: they would say no, and every fade step redraw it all.
        if self.used == 0 {
            return false;
        }
        let mut ok = true;
        for part in &mut self.parts[..self.used] {
            ok &= part.set_alpha(alpha);
        }
        ok
    }

    fn hide(&mut self) {
        self.used = 0;
        for part in &mut self.parts {
            part.hide();
        }
    }
}

/// The tag's text rasterised by GDI: (width, `width * h` bytes of 0..255 coverage).
#[cfg(windows)]
fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    win::label_mask(font_px, h, text)
}

/// macOS: the tag's text drawn by AppKit (`page_overlay::label_mask`).
#[cfg(target_os = "macos")]
fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    crate::page_overlay::label_mask(font_px, h, text)
}

/// Other builds have no text drawing: a tag of about the right width with no ink.
#[cfg(not(any(windows, target_os = "macos")))]
fn label_mask(font_px: u32, h: u32, text: &str) -> (u32, Vec<u8>) {
    let w = (text.chars().count() as u32 * font_px * 3 / 5).max(1);
    (w, vec![0; (w * h) as usize])
}

/// The window side of the cursor, so a test (and `--e2e`) can stand in for it.
pub trait CursorShell {
    /// Shows `bmp` with its tip at (x, y) in screen pixels, at `alpha` (255 opaque) over all. True when
    /// it was shown.
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool;
    /// The picture last shown, where it is, at a new `alpha` (a fade), with no new bitmap. False when
    /// that cannot be done (nothing shown); the caller shows it again whole.
    fn set_alpha(&mut self, alpha: u8) -> bool {
        let _ = alpha;
        false
    }
    /// Gone at once.
    fn hide(&mut self);
}

/// The window side of the pad the person draws an arrow on (a test and `--e2e` stand in for it).
pub trait PadShell {
    /// Covers (x, y, w, h) in screen pixels: all but clear, the mouse's, never the keyboard's. True when shown.
    fn show(&mut self, x: i32, y: i32, w: u32, h: u32) -> bool;
    /// Gone at once.
    fn hide(&mut self);
}

/// The window side of the glow round the page: [`CursorShell`] for an [`EdgeGlow`], whose top left is put
/// at (x, y) in screen pixels.
pub trait GlowShell {
    fn show_at(&mut self, x: i32, y: i32, glow: &EdgeGlow, alpha: u8) -> bool;
    fn set_alpha(&mut self, alpha: u8) -> bool {
        let _ = alpha;
        false
    }
    fn hide(&mut self);
}

/// What the cursor's shell was last told on this thread: the e2e probe and the tests read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorRecord {
    pub shown: bool,
    /// The tip, in screen pixels.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub alpha: u8,
    /// How many times it was drawn.
    pub draws: u64,
}

type Record = std::thread::LocalKey<std::cell::RefCell<CursorRecord>>;

std::thread_local! {
    static RECORD: std::cell::RefCell<CursorRecord> = std::cell::RefCell::new(CursorRecord::default());
    static BORDER_RECORD: std::cell::RefCell<CursorRecord> =
        std::cell::RefCell::new(CursorRecord::default());
    static ARROW_RECORD: std::cell::RefCell<CursorRecord> =
        std::cell::RefCell::new(CursorRecord::default());
    static MARK_RECORD: std::cell::RefCell<CursorRecord> =
        std::cell::RefCell::new(CursorRecord::default());
    static PAD_RECORD: std::cell::RefCell<CursorRecord> =
        std::cell::RefCell::new(CursorRecord::default());
}

/// What a [`RecordArrow`] (the agent's arrow) on this thread was last told: `x`, `y` the head.
pub fn recorded_arrow() -> CursorRecord {
    ARROW_RECORD.with(|r| r.borrow().clone())
}

/// What a [`RecordMark`] (the person's arrow) on this thread was last told: `x`, `y` the head.
pub fn recorded_mark() -> CursorRecord {
    MARK_RECORD.with(|r| r.borrow().clone())
}

/// What a [`RecordPad`] on this thread was last told: `x`, `y` its top left, `w`, `h` its size.
pub fn recorded_pad() -> CursorRecord {
    PAD_RECORD.with(|r| r.borrow().clone())
}

/// The agent's arrow's stand-in, as [`RecordCursor`] is the cursor's.
pub struct RecordArrow;

impl CursorShell for RecordArrow {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        record_show(&ARROW_RECORD, x, y, bmp, alpha)
    }

    fn hide(&mut self) {
        ARROW_RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

/// The person's arrow's stand-in.
pub struct RecordMark;

impl CursorShell for RecordMark {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        record_show(&MARK_RECORD, x, y, bmp, alpha)
    }

    fn hide(&mut self) {
        MARK_RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

/// The pad's stand-in.
pub struct RecordPad;

impl PadShell for RecordPad {
    fn show(&mut self, x: i32, y: i32, w: u32, h: u32) -> bool {
        PAD_RECORD.with(|r| {
            let mut r = r.borrow_mut();
            r.shown = true;
            (r.x, r.y, r.w, r.h, r.alpha) = (x, y, w, h, 1);
            r.draws += 1;
        });
        true
    }

    fn hide(&mut self) {
        PAD_RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

/// What a [`RecordCursor`] on this thread was last told.
pub fn recorded() -> CursorRecord {
    RECORD.with(|r| r.borrow().clone())
}

/// What a [`RecordBorder`] on this thread was last told (`x`, `y` its top left).
pub fn recorded_border() -> CursorRecord {
    BORDER_RECORD.with(|r| r.borrow().clone())
}

fn record_show(rec: &'static Record, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
    rec.with(|r| {
        let mut r = r.borrow_mut();
        r.shown = true;
        (r.x, r.y, r.w, r.h, r.alpha) = (x, y, bmp.w, bmp.h, alpha);
        r.draws += 1;
    });
    true
}

fn record_alpha(rec: &'static Record, alpha: u8) -> bool {
    rec.with(|r| {
        let mut r = r.borrow_mut();
        if r.shown {
            r.alpha = alpha;
        }
        r.shown
    })
}

/// A shell that keeps what it is told and shows it nowhere: tests and `--e2e` runs never make the window.
pub struct RecordCursor;

impl CursorShell for RecordCursor {
    fn show_at(&mut self, x: i32, y: i32, bmp: &CursorBitmap, alpha: u8) -> bool {
        record_show(&RECORD, x, y, bmp, alpha)
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        record_alpha(&RECORD, alpha)
    }

    fn hide(&mut self) {
        RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

/// The glow's stand-in, as [`RecordCursor`] is the arrow's (`w`, `h` the page's).
pub struct RecordBorder;

impl GlowShell for RecordBorder {
    fn show_at(&mut self, x: i32, y: i32, glow: &EdgeGlow, alpha: u8) -> bool {
        BORDER_RECORD.with(|r| {
            let mut r = r.borrow_mut();
            r.shown = true;
            (r.x, r.y, r.w, r.h, r.alpha) = (x, y, glow.w, glow.h, alpha);
            r.draws += 1;
        });
        true
    }

    fn set_alpha(&mut self, alpha: u8) -> bool {
        record_alpha(&BORDER_RECORD, alpha)
    }

    fn hide(&mut self) {
        BORDER_RECORD.with(|r| r.borrow_mut().shown = false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn px(b: &CursorBitmap, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as u32 * b.w + x as u32) * 4) as usize;
        [b.bgra[i], b.bgra[i + 1], b.bgra[i + 2], b.bgra[i + 3]]
    }

    /// The pixel `(ux, uy)` arrow units from the tip.
    fn at_units(b: &CursorBitmap, scale: f64, ux: f64, uy: f64) -> [u8; 4] {
        let k = UNIT * scale;
        px(
            b,
            b.tip.0 + (ux * k).floor() as i32,
            b.tip.1 + (uy * k).floor() as i32,
        )
    }

    #[test]
    fn inside_the_arrow_is_opaque_orange_and_up_left_of_the_tip_is_clear() {
        for s in SCALES {
            let b = compose_cursor(s, "", &[]);
            assert_eq!(b.bgra.len(), (b.w * b.h * 4) as usize);
            assert_eq!(
                at_units(&b, s, 5.0, 8.0),
                [ORANGE.2, ORANGE.1, ORANGE.0, 255],
                "at {s}"
            );
            assert!(px(&b, b.tip.0, b.tip.1)[3] > 0, "the tip is drawn at {s}");
            let off = (3.0 * s).ceil() as i32;
            assert_eq!(px(&b, b.tip.0 - off, b.tip.1 - off), [0; 4], "at {s}");
        }
    }

    #[test]
    fn the_rim_is_white_and_the_edge_is_anti_aliased() {
        let s = 3.0;
        let b = compose_cursor(s, "", &[]);
        // The middle of the top edge, on the path: the rim's centre.
        let p = at_units(&b, s, 7.5, 4.6);
        assert!(
            p[3] == 255 && p[0] > 230 && p[1] > 230 && p[2] > 230,
            "{p:?}"
        );
        let a = arrow_coverage(s);
        assert!(a.outer.iter().any(|c| *c > 0.0 && *c < 1.0));
        assert!(a.fill.iter().zip(&a.outer).all(|(f, o)| f <= o));
        // A pixel's colour never runs past its alpha (premultiplied).
        assert!(
            b.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
        );
    }

    #[test]
    fn a_ripple_is_orange_round_the_tip_and_fades_as_it_grows() {
        for s in SCALES {
            let off = (3.0 * s).round() as i32;
            let alpha = |t: f32| {
                let b = compose_cursor(s, "", &[t]);
                px(&b, b.tip.0 - off, b.tip.1 - off)[3]
            };
            assert_eq!(alpha(1.0), 0, "a finished ripple is gone at {s}");
            let (early, late) = (alpha(0.2), alpha(0.8));
            assert!(early > late && late > 0, "{early} {late} at {s}");
            let b = compose_cursor(s, "", &[0.2]);
            let p = px(&b, b.tip.0 - off, b.tip.1 - off);
            // Orange: red over green over blue.
            assert!(p[2] > p[1] && p[1] > p[0], "{p:?}");
        }
        // The ripple is under the arrow: the arrow's inside stays the same.
        let b = compose_cursor(1.0, "", &[0.1]);
        assert_eq!(
            at_units(&b, 1.0, 5.0, 8.0),
            [ORANGE.2, ORANGE.1, ORANGE.0, 255]
        );
    }

    #[test]
    fn every_scale_fits_the_largest_ripple_and_the_tag() {
        for s in SCALES {
            let b = compose_cursor(s, "Claude", &[0.999]);
            let r = (RIPPLE_PX * 1.4 / 2.0 * s).ceil() as i32;
            assert!(b.tip.0 >= r && b.tip.1 >= r, "at {s}");
            assert!(
                b.w as i32 > b.tip.0 + r && b.h as i32 > b.tip.1 + r,
                "at {s}"
            );
        }
        assert_eq!(clean_scale(f64::NAN), 1.0);
        assert_eq!(clean_scale(9.0), 4.0);
    }

    #[test]
    fn a_long_name_is_cut() {
        assert_eq!(tag_text("  Claude "), "Claude");
        assert_eq!(tag_text(&"x".repeat(100)).len(), TAG_CHARS);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn the_tag_is_an_orange_pill_with_the_name_readable_on_it() {
        for s in [1.0, 1.5, 2.25, 3.0] {
            let b = compose_cursor(s, "Claude", &[]);
            let none = compose_cursor(s, "", &[]);
            assert!(b.w > none.w, "the tag widens the picture at {s}");
            // The tag's rectangle in the picture.
            let x0 = b.tip.0 + (TAG_AT.0 * s).round() as i32;
            let y0 = b.tip.1 + (TAG_AT.1 * s).round() as i32;
            let th = (TAG_H * s).round() as i32;
            let mid = y0 + th / 2;
            // Its left end is orange, the arrow well clear of it.
            let pad = (TAG_PAD * s).round() as i32;
            let mut dark = Vec::new();
            for x in x0..b.w as i32 {
                let p = px(&b, x, mid);
                if p[3] == 255 && p[2] < 0x80 {
                    dark.push(x);
                }
            }
            let ink_w = dark.last().zip(dark.first()).map_or(0, |(l, f)| l - f + 1);
            // Six letters of 11 px Segoe UI semibold are about 36 px wide.
            assert!(
                f64::from(ink_w) > 25.0 * s && f64::from(ink_w) < 55.0 * s,
                "ink {ink_w} px at {s}"
            );
            assert!(dark.first().is_some_and(|f| *f >= x0 + pad - 1), "at {s}");
            let edge = px(&b, x0 + pad / 2, mid);
            assert_eq!(edge, [ORANGE.2, ORANGE.1, ORANGE.0, 255], "at {s}");
        }
    }

    #[test]
    fn the_recording_shell_keeps_what_it_was_told() {
        let mut r = RecordCursor;
        assert!(!recorded().shown);
        let b = compose_cursor(1.0, "Claude", &[]);
        assert!(r.show_at(100, 200, &b, 255));
        let rec = recorded();
        assert!(rec.shown);
        assert_eq!(
            (rec.x, rec.y, rec.w, rec.alpha, rec.draws),
            (100, 200, b.w, 255, 1)
        );
        r.hide();
        assert!(!recorded().shown);
    }

    #[test]
    fn the_glow_is_orange_at_the_page_s_edges_and_fades_smoothly_to_clear_inside() {
        let peak = (255.0 * GLOW_ALPHA).round() as u8;
        for s in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let (w, h) = (400, 300);
            let b = glow_bitmap(w, h, s);
            assert_eq!((b.w, b.h, b.tip), (w, h, (0, 0)));
            assert_eq!(b.bgra.len(), (w * h * 4) as usize);
            let r = (GLOW_PX * s).round() as i32;
            let (wi, hi) = (w as i32, h as i32);
            // Strongest at each edge: the cursor's orange at about half, premultiplied (red over green over blue).
            for (x, y) in [(0, 150), (wi - 1, 150), (200, 0), (200, hi - 1)] {
                let p = px(&b, x, y);
                assert!(
                    p[3] >= peak - 8 && p[3] <= peak,
                    "at the edge at {s}: ({x}, {y}) {p:?}"
                );
                assert!(p[2] > p[1] && p[1] > p[0], "{p:?}");
                assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
            }
            // Fading in from every edge: never rising, no step of more than a few levels, eased, clear at the depth.
            let lines: [Vec<(i32, i32)>; 4] = [
                (0..=r).map(|d| (d, 150)).collect(),
                (0..=r).map(|d| (wi - 1 - d, 150)).collect(),
                (0..=r).map(|d| (200, d)).collect(),
                (0..=r).map(|d| (200, hi - 1 - d)).collect(),
            ];
            for line in &lines {
                let a: Vec<u8> = line.iter().map(|&(x, y)| px(&b, x, y)[3]).collect();
                assert!(a.windows(2).all(|p| p[1] <= p[0]), "falls at {s}: {a:?}");
                assert!(
                    a.windows(2).all(|p| p[0] - p[1] <= 12),
                    "smooth at {s}: {a:?}"
                );
                let mid = a[(r / 2) as usize];
                assert!(mid > 0 && mid < peak / 2, "eased at {s}: {a:?}");
                assert_eq!(a[r as usize], 0, "clear {r} px in at {s}: {a:?}");
            }
            assert_eq!(px(&b, 200, 150), [0; 4], "clear in the middle at {s}");
            // A corner is no stronger than an edge, and the glow round it is smooth and symmetric (no seam).
            assert!(px(&b, 0, 0)[3] >= peak - 1 && px(&b, 0, 0)[3] <= peak);
            for d in 0..r {
                let diag = px(&b, d, d)[3];
                let next = px(&b, d + 1, d)[3];
                assert!(
                    next <= diag && diag - next <= 12,
                    "corner at {s}, {d}: {diag} {next}"
                );
                assert_eq!(px(&b, d, 5), px(&b, 5, d), "symmetric at {s}");
            }
        }
        assert_eq!(edge_glow(0, 10, 1.0).w, 0);
        assert!(edge_glow(GLOW_MAX + 1, 10, 1.0).bands.is_empty());
        // A page thinner than the glow is all glow, and no alpha goes past the peak.
        let b = glow_bitmap(20, 20, 3.0);
        assert!(
            b.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] > 0 && p[3] <= peak)
        );
    }

    #[test]
    fn the_glow_s_bands_are_the_whole_glow_and_nothing_else() {
        // Laid on a clear page, the bands are exactly the whole page's glow: they do not overlap, and what
        // they leave out is clear. Small pages (bands meeting, a side gone) included.
        for (w, h, s) in [
            (300, 200, 1.5),
            (400, 300, 1.0),
            (50, 30, 2.0),
            (20, 200, 1.0),
            (7, 3, 3.0),
        ] {
            let whole = glow_bitmap(w, h, s);
            let g = edge_glow(w, h, s);
            assert_eq!((g.w, g.h), (w, h));
            let mut laid = vec![0u8; (w * h * 4) as usize];
            let mut cover = vec![0u8; (w * h) as usize];
            for ((bx, by), b) in &g.bands {
                for y in 0..b.h {
                    for x in 0..b.w {
                        let (px_, py_) = (*bx as u32 + x, *by as u32 + y);
                        let at = (py_ * w + px_) as usize;
                        cover[at] += 1;
                        let src = ((y * b.w + x) * 4) as usize;
                        laid[at * 4..at * 4 + 4].copy_from_slice(&b.bgra[src..src + 4]);
                    }
                }
            }
            assert!(cover.iter().all(|&c| c <= 1), "overlap at {w}x{h}");
            assert_eq!(laid, whole.bgra, "{w}x{h} at {s}");
        }
        // A large page is four thin bands, not a page-sized bitmap.
        let g = edge_glow(3840, 2160, 2.0);
        let bytes: usize = g.bands.iter().map(|(_, b)| b.bgra.len()).sum();
        assert_eq!(g.bands.len(), 4);
        assert!(bytes < 3840 * 2160 * 4 / 8, "{bytes}");
    }

    #[test]
    #[ignore = "a timing, run by hand: cargo test -p flare-win --release glow_timing -- --ignored --nocapture"]
    fn glow_timing() {
        for (w, h, s) in [(1600, 2000, 2.0), (3840, 2160, 2.0), (3840, 2160, 1.0)] {
            let t = std::time::Instant::now();
            let n = 20;
            for _ in 0..n {
                std::hint::black_box(edge_glow(w, h, s));
            }
            println!("glow {w}x{h} at {s}: {:?} a build", t.elapsed() / n);
        }
    }

    #[test]
    fn the_border_s_stand_in_keeps_its_own_record_and_a_fade_changes_only_the_alpha() {
        let mut r = RecordBorder;
        assert!(!r.set_alpha(10), "nothing shown");
        let b = edge_glow(40, 30, 1.0);
        assert!(r.show_at(5, 6, &b, 255));
        assert!(!recorded().shown, "not the arrow's");
        assert!(r.set_alpha(128));
        let rec = recorded_border();
        assert_eq!(
            (rec.x, rec.y, rec.w, rec.h, rec.alpha, rec.draws),
            (5, 6, 40, 30, 128, 1)
        );
        r.hide();
        assert!(!recorded_border().shown);
    }

    #[cfg(windows)]
    #[test]
    fn the_glow_s_windows_are_not_made_without_an_owner() {
        let mut w = GlowWindow::new(0);
        assert!(!w.show_at(10, 10, &edge_glow(40, 30, 1.0), 255));
        assert!(!w.show_at(10, 10, &edge_glow(0, 0, 1.0), 255));
        w.hide();
    }

    #[cfg(windows)]
    #[test]
    fn the_window_is_not_made_without_an_owner() {
        let mut w = CursorWindow::new(0);
        assert!(!w.show_at(10, 10, &compose_cursor(1.0, "Claude", &[]), 255));
        assert!(!w.set_alpha(100));
        w.hide();
    }

    // ---- arrows (2026-11-10) ----

    /// The pixel `d` pixels from the head back along the arrow (towards its tail).
    fn along(b: &CursorBitmap, tail: (f64, f64), d: f64) -> [u8; 4] {
        let len = (tail.0 * tail.0 + tail.1 * tail.1).sqrt();
        let (x, y) = (
            f64::from(b.tip.0) + tail.0 / len * d,
            f64::from(b.tip.1) + tail.1 / len * d,
        );
        px(b, x.floor() as i32, y.floor() as i32)
    }

    #[test]
    fn an_arrow_is_solid_in_its_ink_along_its_shaft_and_at_its_head() {
        for s in [1.0, 1.5, 2.25, 3.0] {
            for (tail, ink, c) in [
                ((120.0, 80.0), ArrowInk::Agent, ORANGE),
                ((-60.0, 200.0), ArrowInk::Person, BLUE),
                ((0.0, -150.0), ArrowInk::Person, BLUE),
            ] {
                let tail = (tail.0 * s, tail.1 * s);
                let b = compose_arrow(tail, s, ink, "");
                assert_eq!(b.bgra.len(), (b.w * b.h * 4) as usize);
                let len = (tail.0 * tail.0 + tail.1 * tail.1).sqrt();
                // The middle of the shaft and the inside of the head are the ink, opaque.
                let ink = |p: [u8; 4]| {
                    p[3] == 255
                        && [(p[0], c.2), (p[1], c.1), (p[2], c.0)]
                            .iter()
                            .all(|(got, want)| got.abs_diff(*want) <= 12)
                };
                let p = along(&b, tail, len / 2.0);
                assert!(ink(p), "{tail:?} at {s}: {p:?}");
                let p = along(&b, tail, HEAD_LEN_PX * s * 0.6);
                assert!(ink(p), "head {tail:?} at {s}: {p:?}");
                assert!(px(&b, b.tip.0, b.tip.1)[3] > 0, "the head's point is drawn");
                // Beside the shaft, well clear of it, nothing.
                let (nx, ny) = (-tail.1 / len, tail.0 / len);
                let mid = (
                    f64::from(b.tip.0) + tail.0 / 2.0,
                    f64::from(b.tip.1) + tail.1 / 2.0,
                );
                let off = 10.0 * s;
                assert_eq!(
                    px(&b, (mid.0 + nx * off) as i32, (mid.1 + ny * off) as i32),
                    [0; 4],
                    "{tail:?} at {s}"
                );
                assert!(
                    b.bgra
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]),
                    "premultiplied"
                );
            }
        }
    }

    #[test]
    fn a_click_with_no_drag_points_from_up_and_left_and_a_short_one_keeps_its_direction() {
        let b = compose_arrow((0.0, 0.0), 1.0, ArrowInk::Person, "");
        let min = HEAD_LEN_PX * 1.5;
        let d = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(
            along(&b, (d, d), min - 3.0)[3],
            255,
            "drawn down and to the right"
        );
        assert_eq!(
            along(&b, (-d, -d), 10.0)[3],
            0,
            "nothing up and to the left"
        );
        let b = compose_arrow((0.0, 5.0), 1.0, ArrowInk::Person, "");
        assert_eq!(
            along(&b, (0.0, 1.0), min - 3.0)[3],
            255,
            "straight down, at its least length"
        );
        // A long arrow's picture is its own box, not padded to a page.
        let b = compose_arrow((1000.0, 0.0), 1.0, ArrowInk::Agent, "");
        assert!(b.w < 1060 && b.h < 60, "{} x {}", b.w, b.h);
        let huge = compose_arrow((1e9, f64::NAN), 1.0, ArrowInk::Agent, "");
        assert!(huge.w <= ARROW_MAX as u32 + 100, "{}", huge.w);
    }

    #[test]
    fn only_the_agents_arrow_carries_its_label_and_a_long_one_is_cut() {
        let plain = compose_arrow((60.0, 60.0), 1.0, ArrowInk::Agent, "");
        let tagged = compose_arrow((60.0, 60.0), 1.0, ArrowInk::Agent, "Click here to sign in");
        assert!(tagged.w > plain.w && tagged.h > plain.h);
        assert_eq!(tagged.tip, plain.tip, "the head stays where it was");
        let person = compose_arrow((60.0, 60.0), 1.0, ArrowInk::Person, "Click here");
        assert_eq!((person.w, person.h), (plain.w, plain.h));
        let long = compose_arrow((60.0, 60.0), 1.0, ArrowInk::Agent, &"w".repeat(200));
        let cut = compose_arrow(
            (60.0, 60.0),
            1.0,
            ArrowInk::Agent,
            &"w".repeat(POINT_LABEL_CHARS),
        );
        assert_eq!(long.w, cut.w);
    }

    #[test]
    fn the_agents_arrow_turns_its_tail_to_stay_on_the_page() {
        let page = (0, 0, 600, 400);
        let len = 64.0;
        let (dx, dy) = point_tail((300, 200), page, len);
        assert!(dx > 0.0 && dy > 0.0, "down and right in the middle");
        let (dx, dy) = point_tail((590, 200), page, len);
        assert!(dx < 0.0 && dy > 0.0, "left of a head at the right edge");
        let (dx, dy) = point_tail((300, 390), page, len);
        assert!(dx > 0.0 && dy < 0.0, "above a head at the bottom");
        let (dx, dy) = point_tail((595, 395), page, len);
        assert!(dx < 0.0 && dy < 0.0, "up and left in the far corner");
        assert!(((dx * dx + dy * dy).sqrt() - len).abs() < 0.01);
    }

    #[test]
    fn an_arrow_laid_on_a_screenshot_lands_at_its_head_and_stops_at_the_edges() {
        let (w, h) = (200u32, 100u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        let b = compose_arrow((40.0, 30.0), 1.0, ArrowInk::Person, "");
        blend_onto_rgba(&mut rgba, w, h, (50, 40), &b);
        // The shaft's middle, laid at (50, 40) + half the tail: the person's blue.
        let (x, y) = (50 + 20, 40 + 15);
        let i = ((y * w + x) * 4) as usize;
        assert_eq!(&rgba[i..i + 4], &[BLUE.0, BLUE.1, BLUE.2, 255]);
        // Far from it, the picture is as it was.
        let i = ((5 * w + 190) * 4) as usize;
        assert_eq!(&rgba[i..i + 4], &[255, 255, 255, 255]);
        // Half off the picture is drawn where it falls and nowhere else (no panic, nothing wraps).
        let mut small = vec![0u8; 10 * 10 * 4];
        blend_onto_rgba(&mut small, 10, 10, (-5, -5), &b);
        blend_onto_rgba(&mut small, 10, 10, (500, 500), &b);
        let mut wrong = vec![0u8; 7];
        blend_onto_rgba(&mut wrong, 10, 10, (0, 0), &b);
    }

    #[test]
    fn the_arrow_and_pad_stand_ins_keep_their_own_records() {
        let b = compose_arrow((60.0, 60.0), 1.0, ArrowInk::Agent, "x");
        let mut a = RecordArrow;
        assert!(a.show_at(10, 20, &b, 255));
        assert!(recorded_arrow().shown && !recorded_mark().shown && !recorded().shown);
        assert_eq!((recorded_arrow().x, recorded_arrow().y), (10, 20));
        a.hide();
        assert!(!recorded_arrow().shown);
        let mut m = RecordMark;
        assert!(m.show_at(1, 2, &b, 255));
        assert!(recorded_mark().shown && !recorded_arrow().shown);
        m.hide();
        let mut p = RecordPad;
        assert!(p.show(5, 6, 300, 200));
        let r = recorded_pad();
        assert_eq!((r.shown, r.x, r.y, r.w, r.h), (true, 5, 6, 300, 200));
        p.hide();
        assert!(!recorded_pad().shown);
    }

    #[cfg(windows)]
    #[test]
    fn the_pad_and_arrow_windows_are_not_made_without_an_owner() {
        let mut p = PadWindow::new(0);
        assert!(!p.show(0, 0, 100, 100));
        assert!(!p.show(0, 0, 0, 100));
        p.hide();
        let mut a = ArrowWindow::new(0);
        assert!(!a.show_at(
            0,
            0,
            &compose_arrow((50.0, 50.0), 1.0, ArrowInk::Agent, ""),
            255
        ));
        a.hide();
    }

    /// The probe of 2026-11-10 (docs/surfaces/decisions.md, "Arrows"): can an owned layered window over the page catch
    /// the mouse without taking the keyboard from the page? Run on a desktop of its own that is never shown (nothing
    /// reaches the person's screen): an owner window, a child window in the page's place holding the focus, and the
    /// pad over it.
    #[cfg(windows)]
    #[test]
    #[ignore = "the overlay probe, run by hand: cargo test -p flare-win --lib pad_probe -- --ignored --nocapture"]
    fn pad_probe() {
        let out = std::thread::spawn(probe::run)
            .join()
            .expect("the probe ran");
        println!("{out}");
        assert!(out.contains("RESULT: pass"), "{out}");
    }

    #[cfg(windows)]
    mod probe {
        use super::super::{
            ArrowInk, ArrowWindow, CursorShell, PadShell, PadWindow, compose_arrow, pad_heard,
        };
        use std::ffi::c_void;
        use std::ptr::{null, null_mut};
        use std::sync::atomic::{AtomicU32, Ordering};
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetActiveWindow, GetCapture, GetFocus, SetActiveWindow, SetFocus,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            HTCLIENT, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW,
            SendMessageW, TranslateMessage, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
            WM_MOUSEMOVE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WindowFromPoint,
        };

        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(
                name: *const u16,
                device: *const u16,
                mode: *const c_void,
                flags: u32,
                access: u32,
                sa: *const c_void,
            ) -> *mut c_void;
            fn SetThreadDesktop(desk: *mut c_void) -> i32;
            fn CloseDesktop(desk: *mut c_void) -> i32;
        }
        const GENERIC_ALL: u32 = 0x1000_0000;

        static PAGE_PRESSES: AtomicU32 = AtomicU32::new(0);

        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(Some(0)).collect()
        }

        unsafe extern "system" fn page_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            if m == WM_LBUTTONDOWN {
                PAGE_PRESSES.fetch_add(1, Ordering::SeqCst);
            }
            // SAFETY: the default handling of the window's own message.
            unsafe { DefWindowProcW(h, m, w, l) }
        }

        unsafe extern "system" fn plain_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            // SAFETY: as above.
            unsafe { DefWindowProcW(h, m, w, l) }
        }

        fn pump() {
            // SAFETY: this thread's own queue.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        fn class(
            name: &str,
            proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        ) -> Vec<u16> {
            let n = wide(name);
            // SAFETY: the name outlives the windows of the probe; the procedure is ours.
            unsafe {
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(null()),
                    hIcon: null_mut(),
                    hCursor: null_mut(),
                    hbrBackground: null_mut(),
                    lpszMenuName: null(),
                    lpszClassName: n.as_ptr(),
                };
                RegisterClassW(&wc);
            }
            n
        }

        pub fn run() -> String {
            let mut out = Vec::new();
            let name = wide(&format!("FlarePadProbe{}", std::process::id()));
            // SAFETY: a desktop of this probe's own, never switched to (never shown); this new thread has no window yet.
            let desk =
                unsafe { CreateDesktopW(name.as_ptr(), null(), null(), 0, GENERIC_ALL, null()) };
            // SAFETY: the desktop made just above, for this thread, which has no window yet.
            if desk.is_null() || unsafe { SetThreadDesktop(desk) } == 0 {
                return "RESULT: not run (no hidden desktop could be made)".to_string();
            }
            let owner_class = class("FlarePadProbeOwner", plain_proc);
            let page_class = class("FlarePadProbePage", page_proc);
            // SAFETY: windows of this thread, on the hidden desktop.
            let (owner, page) = unsafe {
                let owner = CreateWindowExW(
                    0,
                    owner_class.as_ptr(),
                    owner_class.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    100,
                    100,
                    900,
                    700,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                let page = CreateWindowExW(
                    0,
                    page_class.as_ptr(),
                    page_class.as_ptr(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    800,
                    560,
                    owner,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                SetActiveWindow(owner);
                SetFocus(page);
                (owner, page)
            };
            pump();
            // SAFETY: plain reads of this thread's windows.
            let (focus0, active0) = unsafe { (GetFocus(), GetActiveWindow()) };
            out.push(format!(
                "setup: the page has the focus {} and the owner is active {}",
                focus0 == page,
                active0 == owner
            ));
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let mut tl = POINT { x: 0, y: 0 };
            // SAFETY: plain reads of the page's place.
            unsafe {
                GetClientRect(page, &mut rc);
                ClientToScreen(page, &mut tl);
            }
            let (w, h) = ((rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32);
            let mut pad = PadWindow::new(owner as isize);
            let shown = pad.show(tl.x, tl.y, w, h);
            pump();
            let ph = pad.hwnd() as HWND;
            let mid = POINT {
                x: tl.x + w as i32 / 2,
                y: tl.y + h as i32 / 2,
            };
            // The agent's arrow over the middle: it lets the mouse through to the pad.
            let mut arrow = ArrowWindow::new(owner as isize);
            let arrow_shown = arrow.show_at(
                mid.x,
                mid.y,
                &compose_arrow((60.0, 60.0), 1.0, ArrowInk::Agent, ""),
                255,
            );
            pump();
            // SAFETY: plain reads.
            let (hit_mid, hit_out, focus1, active1) = unsafe {
                (
                    WindowFromPoint(mid),
                    WindowFromPoint(POINT {
                        x: tl.x + w as i32 + 20,
                        y: mid.y,
                    }),
                    GetFocus(),
                    GetActiveWindow(),
                )
            };
            let hit = shown && hit_mid == ph;
            out.push(format!(
                "hit test: the pad was shown {shown}, the arrow {arrow_shown}; the pad is under the pointer over the page {hit}; beside the page it is not {}",
                hit_out != ph
            ));
            let keeps = focus1 == page && active1 == owner;
            out.push(format!(
                "focus: after the pad came up the page still has it {keeps}"
            ));
            // SAFETY: what Windows asks a window before a press activates it, asked of the pad.
            let ma = unsafe {
                SendMessageW(
                    ph,
                    WM_MOUSEACTIVATE,
                    owner as WPARAM,
                    ((WM_LBUTTONDOWN as isize) << 16 | HTCLIENT as isize) as LPARAM,
                )
            };
            let no_activate = ma == MA_NOACTIVATE as LRESULT;
            out.push(format!(
                "activation: the pad answers no-activate {no_activate}"
            ));
            let (d0, m0, u0) = pad_heard();
            // SAFETY: a press, a drag and a release posted to the pad, as Windows would deliver them.
            unsafe {
                PostMessageW(ph, WM_LBUTTONDOWN, 1, 10 << 16 | 10);
                for i in 1..=3 {
                    PostMessageW(ph, WM_MOUSEMOVE, 1, (10 + 20 * i) << 16 | (10 + 30 * i));
                }
                PostMessageW(ph, WM_LBUTTONUP, 0, 70 << 16 | 100);
            }
            pump();
            let (d1, m1, u1) = pad_heard();
            // SAFETY: plain reads.
            let (focus2, capture) = unsafe { (GetFocus(), GetCapture()) };
            let heard = (d1 - d0, m1 - m0, u1 - u0) == (1, 3, 1);
            let page_quiet = PAGE_PRESSES.load(Ordering::SeqCst) == 0;
            let kept = focus2 == page && capture.is_null();
            out.push(format!(
                "drag: the pad heard press, 3 moves, release {heard}; the page heard no press {page_quiet}; the page kept the focus and the capture was let go {kept}"
            ));
            pad.hide();
            pump();
            // SAFETY: plain read.
            let back = unsafe { WindowFromPoint(mid) } == page;
            out.push(format!(
                "hidden: the page is under the pointer again {back}"
            ));
            let pass = hit && keeps && no_activate && heard && page_quiet && kept && back;
            out.push(format!("RESULT: {}", if pass { "pass" } else { "fail" }));
            drop(arrow);
            drop(pad);
            // SAFETY: this thread's own windows and desktop.
            unsafe {
                DestroyWindow(owner);
                CloseDesktop(desk);
            }
            out.join("\n")
        }
    }
}
