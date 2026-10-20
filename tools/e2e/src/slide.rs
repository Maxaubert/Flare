//! The numbers of a slide (the visual tour, audit 2026-10-20): from the per-frame series the app keeps
//! (`frames.series`: age, gap, render time, the sidebar's slot width and panel x at each frame), the
//! frames the slide took, the longest gap between them, the time it took and how close its curve is to the
//! CSS ease the two apps declare (`cubic-bezier(0.16, 1, 0.3, 1)` over 240 ms).
//!
//! Pure: no app, no clock. Prism Terminal's page numbers go through the same definitions
//! (`tools/pt-shot/pt-visual.mjs` mirrors `analyse`), so the two columns of the report mean the same.

use serde_json::{json, Value};

/// The declared curve and time of the sidebar slide in both apps.
pub const EASE: (f64, f64, f64, f64) = (0.16, 1.0, 0.3, 1.0);
pub const SLIDE_MS: f64 = 240.0;
/// A move smaller than this (css px) is not the slide starting; a value within it of the end is arrived.
pub const MOVED_PX: f64 = 0.25;
pub const ARRIVED_PX: f64 = 0.5;

/// One presented frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub age: f64,
    pub gap: Option<f64>,
    pub render: Option<f64>,
    pub slot: Option<f64>,
    pub left: Option<f64>,
}

/// Rows out of the `frames.series` answer.
pub fn parse_rows(v: &Value) -> Vec<Row> {
    let Some(list) = v["frames"].as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|r| {
            let a = r.as_array()?;
            Some(Row {
                age: a.first()?.as_f64()?,
                gap: a.get(1).and_then(Value::as_f64),
                render: a.get(2).and_then(Value::as_f64),
                slot: a.get(3).and_then(Value::as_f64),
                left: a.get(4).and_then(Value::as_f64),
            })
        })
        .collect()
}

/// CSS `cubic-bezier(x1, y1, x2, y2)`: the progress at time fraction `t` (0..=1).
pub fn bezier(c: (f64, f64, f64, f64), t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let at = |a: f64, b: f64, s: f64| {
        3.0 * a * (1.0 - s).powi(2) * s + 3.0 * b * (1.0 - s) * s * s + s.powi(3)
    };
    // Solve x(s) = t by bisection (x is monotonic for x1, x2 in 0..=1).
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if at(c.0, c.2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(c.1, c.3, (lo + hi) / 2.0)
}

/// What one slide came to.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    /// Frames from the one before the first move to the one that arrived.
    pub frames: usize,
    /// The start value and the end value (css px).
    pub from: f64,
    pub to: f64,
    /// Key press (app age) to the first frame that moved.
    pub input_ms: Option<f64>,
    /// First moved frame to progress 50, 90 and 99 percent, and to arrival (within half a px).
    pub t50: f64,
    pub t90: f64,
    pub t99: f64,
    pub settle: f64,
    /// The longest gap between two presented frames inside the slide, and how many were over 20 and 33 ms.
    pub longest_gap: f64,
    pub over_20: usize,
    pub over_33: usize,
    /// The worst render time of a frame inside the slide.
    pub max_render: f64,
    /// The largest distance (progress, 0..1) between the frames and the declared curve, the first moved
    /// frame taken as time zero, and the best the curve fits with the start slid by up to 30 ms.
    pub dev_at_first: f64,
    pub dev_best: f64,
    pub best_offset_ms: f64,
    /// `(ms since the first moved frame, progress)` per frame.
    pub curve: Vec<(f64, f64)>,
}

impl Stats {
    pub fn to_json(&self) -> Value {
        let r = |v: f64| (v * 100.0).round() / 100.0;
        json!({
            "frames": self.frames, "from": self.from, "to": self.to,
            "input_ms": self.input_ms.map(r),
            "t50_ms": r(self.t50), "t90_ms": r(self.t90), "t99_ms": r(self.t99), "settle_ms": r(self.settle),
            "longest_gap_ms": r(self.longest_gap), "over_20": self.over_20, "over_33": self.over_33,
            "max_render_ms": r(self.max_render),
            "dev_at_first": (self.dev_at_first * 1000.0).round() / 1000.0,
            "dev_best": (self.dev_best * 1000.0).round() / 1000.0,
            "best_offset_ms": self.best_offset_ms,
            "curve": self.curve.iter().map(|(t, p)| json!([r(*t), (p * 1000.0).round() / 1000.0])).collect::<Vec<_>>(),
        })
    }
}

/// The time the progress first reached `p` (linear between frames), counted from `t0`.
fn crossing(curve: &[(f64, f64)], p: f64) -> f64 {
    for w in curve.windows(2) {
        let ((ta, pa), (tb, pb)) = (w[0], w[1]);
        if pa < p && pb >= p {
            let k = if (pb - pa).abs() < 1e-9 {
                1.0
            } else {
                (p - pa) / (pb - pa)
            };
            return ta + k * (tb - ta);
        }
    }
    curve.last().map_or(0.0, |c| c.0)
}

/// Analyses the slide in `rows` for the value `pick` reads from a frame, the key having been pressed at
/// `press_age`. None when the value never moved.
pub fn analyse(rows: &[Row], pick: fn(&Row) -> Option<f64>, press_age: f64) -> Option<Stats> {
    let vals: Vec<(usize, f64)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| pick(r).map(|v| (i, v)))
        .collect();
    let (_, from) = *vals.first()?;
    let (_, to) = *vals.last()?;
    if (to - from).abs() < ARRIVED_PX {
        return None;
    }
    let i0 = vals.iter().find(|(_, v)| (v - from).abs() > MOVED_PX)?.0;
    let i_end = vals
        .iter()
        .rev()
        .find(|(_, v)| (v - to).abs() > ARRIVED_PX)
        .map_or(i0, |(i, _)| *i + 1)
        .min(rows.len() - 1)
        .max(i0);
    let t0 = rows[i0].age;
    let progress = |v: f64| (v - from) / (to - from);
    let curve: Vec<(f64, f64)> = (i0.saturating_sub(1)..=i_end)
        .filter_map(|i| Some((rows[i].age - t0, progress(pick(&rows[i])?))))
        .collect();
    let inside = &rows[i0..=i_end];
    let gaps: Vec<f64> = inside.iter().filter_map(|r| r.gap).collect();
    let dev = |off: f64| {
        curve
            .iter()
            .map(|(t, p)| (p - bezier(EASE, (t - off) / SLIDE_MS)).abs())
            .fold(0.0, f64::max)
    };
    let mut best = (f64::MAX, 0.0);
    for off in -30..=30 {
        let d = dev(f64::from(off));
        if d < best.0 {
            best = (d, f64::from(off));
        }
    }
    Some(Stats {
        frames: i_end + 1 - i0.saturating_sub(1),
        from,
        to,
        input_ms: Some(t0 - press_age),
        t50: crossing(&curve, 0.5),
        t90: crossing(&curve, 0.9),
        t99: crossing(&curve, 0.99),
        settle: rows[i_end].age - t0,
        longest_gap: gaps.iter().copied().fold(0.0, f64::max),
        over_20: gaps.iter().filter(|g| **g > 20.0).count(),
        over_33: gaps.iter().filter(|g| **g > 33.0).count(),
        max_render: inside.iter().filter_map(|r| r.render).fold(0.0, f64::max),
        dev_at_first: dev(0.0),
        dev_best: best.0,
        best_offset_ms: best.1,
        curve,
    })
}

/// The slot-width reader for `analyse`.
pub fn slot(r: &Row) -> Option<f64> {
    r.slot
}

/// The panel-x reader for `analyse`.
pub fn left(r: &Row) -> Option<f64> {
    r.left
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60 fps run of the declared slide from `from` to `to`, starting at `start` (ms), with an idle
    /// frame before it and one after.
    fn run(from: f64, to: f64, start: f64, step: f64) -> Vec<Row> {
        let mut rows = vec![Row {
            age: start - step,
            gap: None,
            render: Some(1.0),
            slot: Some(from),
            left: Some(0.0),
        }];
        let mut t = 0.0;
        while t <= SLIDE_MS + step {
            let p = bezier(EASE, t / SLIDE_MS);
            rows.push(Row {
                age: start + t,
                gap: Some(step),
                render: Some(1.5),
                slot: Some(from + (to - from) * p),
                left: Some(0.0),
            });
            t += step;
        }
        rows
    }

    #[test]
    fn the_declared_curve_starts_at_zero_ends_at_one_and_leaps_early() {
        assert!(bezier(EASE, 0.0).abs() < 1e-9);
        assert!((bezier(EASE, 1.0) - 1.0).abs() < 1e-9);
        // An ease-out: more than half done at a fifth of the time.
        assert!(bezier(EASE, 0.2) > 0.5, "{}", bezier(EASE, 0.2));
    }

    #[test]
    fn a_clean_sixty_fps_slide_fits_the_declared_curve() {
        let rows = run(220.0, 0.0, 1000.0, 1000.0 / 60.0);
        let s = analyse(&rows, slot, 1000.0).expect("it moved");
        assert!(s.dev_best < 0.02, "{}", s.dev_best);
        assert!(s.longest_gap < 17.0, "{}", s.longest_gap);
        assert_eq!(s.over_33, 0);
        assert!(s.t50 < 60.0, "an ease-out is half done early: {}", s.t50);
        assert!(s.t99 > s.t90 && s.t90 > s.t50);
        assert!(s.input_ms.unwrap() > 0.0 && s.input_ms.unwrap() < 20.0);
        assert!(s.frames >= 12, "{}", s.frames);
    }

    #[test]
    fn a_stall_in_the_middle_shows_as_the_longest_gap() {
        let mut rows = run(220.0, 0.0, 1000.0, 1000.0 / 60.0);
        // Drop six frames: the next one arrives 100 ms after its predecessor.
        let at = 5;
        let gap_to = rows[at + 6].age - rows[at - 1].age;
        rows.drain(at..at + 6);
        rows[at].gap = Some(gap_to);
        let s = analyse(&rows, slot, 990.0).unwrap();
        assert!(s.longest_gap > 90.0, "{}", s.longest_gap);
        assert!(s.over_33 >= 1);
    }

    #[test]
    fn a_value_that_never_moved_has_no_slide() {
        let rows = vec![
            Row {
                age: 1.0,
                gap: None,
                render: None,
                slot: Some(5.0),
                left: None,
            },
            Row {
                age: 17.0,
                gap: Some(16.0),
                render: None,
                slot: Some(5.0),
                left: None,
            },
        ];
        assert!(analyse(&rows, slot, 0.0).is_none());
        assert!(analyse(&[], slot, 0.0).is_none());
    }

    #[test]
    fn rows_are_read_from_the_series_answer() {
        let v = json!({"frames": [[10.0, null, 1.5, 220.0, 0.0], [26.5, 16.5, 2.0, 200.0, -3.0]], "now_age_ms": 30.0});
        let rows = parse_rows(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].gap, None);
        assert_eq!(rows[1].slot, Some(200.0));
        assert_eq!(rows[1].left, Some(-3.0));
        assert!(parse_rows(&json!({})).is_empty());
    }
}
