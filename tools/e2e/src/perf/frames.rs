//! B7, frame time at 225% (Task 60, D5), and the frame statistics the flood and scroll measures
//! share.
//!
//! The window is set to 225% (DPI 216), then sized to 1000 x 640 css (2250 x 1440 device px) and
//! 1707 x 960 css (3840 x 2160): during the flood of B3 and during a 3 s continuous resize (a
//! `Resize` every 16 ms between 800 x 500 and the full size), the render time per frame (build and
//! submit, never the vsync wait) must stay at or under 8.3 ms at the 95th percentile and no
//! presented-frame interval over 33 ms. GPU renderer only.

use std::time::{Duration, Instant};

use flare_e2e_proto::{Reply, Request};
use serde_json::{json, Value};

use super::flood::{run_flood, FloodResult};
use super::{launch_ready, pause, Measure, PerfOpts};
use crate::ctx::App;

/// The two window sizes, in css pixels.
pub const SIZES: [(f32, f32); 2] = [(1000.0, 640.0), (1707.0, 960.0)];
/// The smallest size of the resize drag.
pub const DRAG_MIN: (f32, f32) = (800.0, 500.0);
/// How long the drag lasts.
pub const DRAG_MS: u64 = 3_000;
/// The gap between two resizes.
pub const DRAG_STEP_MS: u64 = 16;

/// The `Frames` reply as numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FramesReply {
    pub count: u32,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub over_33: u32,
    pub render_p95_ms: f64,
}

impl FramesReply {
    pub fn from_reply(r: &Reply) -> Option<FramesReply> {
        match r {
            Reply::Frames {
                count,
                mean_ms,
                p50_ms,
                p95_ms,
                p99_ms,
                max_ms,
                over_33,
                render_p95_ms,
            } => Some(FramesReply {
                count: *count,
                mean_ms: *mean_ms,
                p50_ms: *p50_ms,
                p95_ms: *p95_ms,
                p99_ms: *p99_ms,
                max_ms: *max_ms,
                over_33: *over_33,
                render_p95_ms: *render_p95_ms,
            }),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "count": self.count, "mean_ms": self.mean_ms, "p50_ms": self.p50_ms,
            "p95_ms": self.p95_ms, "p99_ms": self.p99_ms, "max_ms": self.max_ms,
            "over_33": self.over_33, "render_p95_ms": self.render_p95_ms,
        })
    }
}

/// Clears the frame ring (`reset`) or reads the statistics since the last reset.
pub fn frames(app: &mut App, reset: bool) -> Result<Option<FramesReply>, String> {
    let reply = app.session.call(&Request::Frames { reset })?;
    Ok(FramesReply::from_reply(&reply))
}

/// The sizes of a resize drag: `steps` sizes sliding down from `full` to `DRAG_MIN` and back, one
/// every 16 ms for 3 s.
pub fn drag_sizes(full: (f32, f32)) -> Vec<(f32, f32)> {
    let steps = (DRAG_MS / DRAG_STEP_MS) as usize;
    (0..steps)
        .map(|i| {
            // A triangle wave: 0 -> 1 -> 0 over the drag.
            let t = i as f32 / (steps - 1) as f32;
            let k = 1.0 - (2.0 * t - 1.0).abs();
            (
                full.0 - (full.0 - DRAG_MIN.0) * k,
                full.1 - (full.1 - DRAG_MIN.1) * k,
            )
        })
        .collect()
}

/// Resizes the window through the drag sizes on a fixed schedule and returns the frame statistics
/// of the whole drag.
pub fn resize_drag(app: &mut App, full: (f32, f32)) -> Result<Option<FramesReply>, String> {
    frames(app, true)?;
    let start = Instant::now();
    for (i, (w, h)) in drag_sizes(full).into_iter().enumerate() {
        let due = start + Duration::from_millis(DRAG_STEP_MS * i as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            pause(wait.as_millis() as u64);
        }
        app.session.call(&Request::Resize { w, h })?;
    }
    frames(app, false)
}

/// The Resize that puts the window at `size` and the wait until it holds.
fn set_size(app: &mut App, size: (f32, f32)) -> Result<(), String> {
    app.session.call(&Request::Resize {
        w: size.0,
        h: size.1,
    })?;
    pause(300);
    Ok(())
}

/// B7 for one renderer (GPU only: the software renderer has no bar here).
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    if renderer != "gpu" {
        return Ok(Vec::new());
    }
    let mut run = launch_ready(o, renderer, Some(o.scale_dpi))?;
    let tab = run.tab.clone();
    let mut out = Vec::new();
    for size in SIZES {
        set_size(&mut run.app, size)?;
        let dir = run.ctx.ensure_world().root.clone();
        let flood: FloodResult = run_flood(&mut run.app, &tab, &dir)?;
        out.push(Measure {
            id: "B7",
            renderer,
            value: flood.frames.render_p95_ms,
            unit: "ms",
            detail: json!({
                "phase": "flood",
                "css_size": [size.0, size.1],
                "scale_dpi": o.scale_dpi,
                "max_ms": flood.frames.max_ms,
                "frames": flood.frames.to_json(),
            }),
        });
        set_size(&mut run.app, size)?;
        let drag = resize_drag(&mut run.app, size)?
            .ok_or_else(|| "the Frames request gave no statistics".to_string())?;
        out.push(Measure {
            id: "B7",
            renderer,
            value: drag.render_p95_ms,
            unit: "ms",
            detail: json!({
                "phase": "resize",
                "css_size": [size.0, size.1],
                "scale_dpi": o.scale_dpi,
                "max_ms": drag.max_ms,
                "frames": drag.to_json(),
            }),
        });
    }
    run.app.close();
    Ok(out)
}
