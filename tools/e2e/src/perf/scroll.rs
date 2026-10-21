//! B5, scroll throughput (Task 60, D5): 100,000 lines in the scrollback (a file shown with `type`,
//! never a `for /L` loop), then 5000 wheel events one millisecond apart. The presented frames per
//! second and the longest interval between two frames are read from the app.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{PointerAction, Request};
use serde_json::json;

use super::frames::frames;
use super::{launch_ready, pause, Measure, PerfOpts};
use crate::ctx::App;
use crate::until::{wait_on, SystemClock};
use crate::world::rows_text;

/// Lines in the scrollback.
pub const LINES: usize = 100_000;
/// Wheel events and their spacing: 1000 a second for 5 s.
pub const WHEEL_EVENTS: usize = 5_000;
pub const WHEEL_GAP_MS: u64 = 1;
/// One notch of the wheel.
pub const WHEEL_DY: f32 = -120.0;
/// The last line of the file.
pub const SCROLL_END: &str = "SCROLL-END";

/// Writes the 100,000-line file (`line 000001 ...`, the last line `SCROLL-END`).
pub fn scroll_file(dir: &Path) -> PathBuf {
    let path = dir.join("scroll-100k.txt");
    let file = std::fs::File::create(&path).expect("create the scroll file");
    let mut out = BufWriter::new(file);
    for i in 1..LINES {
        writeln!(
            out,
            "line {i:06} the quick brown fox jumps over the lazy dog"
        )
        .expect("write");
    }
    writeln!(out, "{SCROLL_END}").expect("write");
    out.flush().expect("flush");
    path
}

/// Frames per second of `count` presented frames over `seconds`.
pub fn fps(count: u32, seconds: f64) -> f64 {
    if seconds > 0.0 {
        f64::from(count) / seconds
    } else {
        0.0
    }
}

/// The grid's centre in css px, from the `ui.rect` probe.
fn grid_centre(app: &mut App) -> (f32, f32) {
    let g = app
        .session
        .probe("ui.rect", json!({ "name": "grid" }))
        .unwrap_or_default();
    let f = |k: &str| g[k].as_f64().unwrap_or(0.0) as f32;
    (f("x") + f("w") / 2.0, f("y") + f("h") / 2.0)
}

/// B5 for one renderer.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let mut run = launch_ready(o, renderer, None)?;
    let tab = run.tab.clone();
    let dir = run.ctx.ensure_world().root.clone();
    let file = scroll_file(&dir);
    run.app
        .session
        .type_text(&format!("type \"{}\"", file.display()))?;
    run.app.session.key("Enter")?;
    let done = wait_on(&mut SystemClock::new(), 120_000, &mut || {
        rows_text(&mut run.app, &tab).contains(SCROLL_END)
    });
    if !done {
        return Err("the 100,000 lines never finished".into());
    }
    pause(500);
    let (x, y) = grid_centre(&mut run.app);
    run.app.session.call(&Request::Pointer {
        x,
        y,
        action: PointerAction::Move,
    })?;
    frames(&mut run.app, true)?;
    let start = Instant::now();
    for i in 0..WHEEL_EVENTS {
        let due = start + Duration::from_millis(WHEEL_GAP_MS * i as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait); // sync-ok: test harness, off the app
        }
        run.app.session.call(&Request::Pointer {
            x,
            y,
            action: PointerAction::Wheel {
                dx: 0.0,
                dy: WHEEL_DY,
            },
        })?;
    }
    let seconds = start.elapsed().as_secs_f64();
    let stats = frames(&mut run.app, false)?
        .ok_or_else(|| "the Frames request gave no statistics".to_string())?;
    run.app.close();
    Ok(vec![Measure {
        id: "B5",
        renderer,
        value: fps(stats.count, seconds),
        unit: "fps",
        detail: json!({
            "seconds": seconds,
            "events": WHEEL_EVENTS,
            "max_ms": stats.max_ms,
            "over_33": stats.over_33,
            "frames": stats.to_json(),
        }),
    }])
}
