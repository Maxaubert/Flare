//! B3, heavy-output throughput (Task 60, D5): `type flood.txt` where the file is 100 MB of
//! 100-byte lines ending in `FLOOD-END`; the time from the Enter key to the first presented frame
//! that holds `FLOOD-END`, and the frame statistics from 500 ms after the Enter to the end.
//!
//! The app times the end of the flood itself (`Latency`, on its own clock). The start is the
//! harness's Enter: the frame statistics are reset 500 ms later, and the `Latency` request that
//! waits for the marker starts then, so the reported seconds are the 500 ms plus the app's own
//! wait. A flood that ends inside the first 500 ms is flagged `short` in the detail.

use std::path::{Path, PathBuf};
use std::time::Instant;

use flare_e2e_proto::{Reply, Request};
use serde_json::json;

use super::frames::{frames, FramesReply};
use super::{launch_ready, pause, Measure, PerfOpts};
use crate::ctx::App;
use crate::standins::flood::flood_file;
use crate::until::{wait_on, Clock, SystemClock};
use crate::world::{prompt_showing, rows_text, Shell};

/// The size of the flood.
pub const FLOOD_MB: usize = 100;
/// The last line of the flood file.
pub const FLOOD_END: &str = "FLOOD-END";
/// Frames count from this long after the Enter.
pub const FRAMES_START_MS: u64 = 500;
/// The longest a flood may take.
const FLOOD_TIMEOUT_MS: u64 = 120_000;

/// What one flood gave.
#[derive(Clone, Debug, PartialEq)]
pub struct FloodResult {
    pub mb: f64,
    pub seconds: f64,
    pub mb_per_s: f64,
    pub frames: FramesReply,
}

/// Throughput in MB per second.
pub fn mb_per_s(mb: f64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        mb / seconds
    } else {
        0.0
    }
}

/// Waits (on `clock`, which started at the Enter) until `FRAMES_START_MS` have passed.
pub fn wait_frames_start(clock: &mut dyn Clock) {
    loop {
        let elapsed = clock.elapsed_ms();
        if elapsed >= FRAMES_START_MS {
            return;
        }
        clock.sleep_ms(FRAMES_START_MS - elapsed);
    }
}

/// The flood file in `dir`, written once.
pub fn ensure_flood_file(dir: &Path) -> PathBuf {
    let path = dir.join(format!("flood-{FLOOD_MB}mb.txt"));
    let want = (FLOOD_MB * 1024 * 1024) as u64;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == want) {
        return path;
    }
    flood_file(dir, FLOOD_MB, FLOOD_END)
}

/// Clears whatever is on the prompt line and the screen, and waits for a clean prompt.
fn clean_prompt(app: &mut App, tab: &str) -> Result<(), String> {
    app.session.key("Escape")?;
    app.session.type_text("cls")?;
    app.session.key("Enter")?;
    let clean = wait_on(&mut SystemClock::new(), 15_000, &mut || {
        let t = rows_text(app, tab);
        !t.contains(FLOOD_END) && prompt_showing(&t, Shell::Cmd)
    });
    if clean {
        Ok(())
    } else {
        Err("the screen did not clear before the flood".into())
    }
}

/// Floods the front tab once and measures it. The tab is at a `cmd` prompt.
pub fn run_flood(app: &mut App, tab: &str, dir: &Path) -> Result<FloodResult, String> {
    let file = ensure_flood_file(dir);
    clean_prompt(app, tab)?;
    app.session
        .type_text(&format!("type \"{}\"", file.display()))?;
    let mut clock = SystemClock::new();
    let began = Instant::now();
    app.session.key("Enter")?;
    wait_frames_start(&mut clock);
    frames(app, true)?;
    let waited = began.elapsed().as_secs_f64();
    // Any key starts the wait; the shift-Z lands in the line after the flood and `clean_prompt`
    // clears it.
    let ms = match app.session.call(&Request::Latency {
        chord: "Shift+KeyZ".to_string(),
        expect: FLOOD_END.to_string(),
        // Latency reads the front tab only; the only tab is the front one.
        tab: None,
        timeout_ms: FLOOD_TIMEOUT_MS,
    })? {
        Reply::Latency { ms } => ms,
        other => return Err(format!("expected Latency, got {other:?}")),
    };
    app.session.call(&Request::Key {
        chord: "Shift+KeyZ".to_string(),
        down: false,
    })?;
    let stats =
        frames(app, false)?.ok_or_else(|| "the Frames request gave no statistics".to_string())?;
    let seconds = waited + ms / 1000.0;
    Ok(FloodResult {
        mb: FLOOD_MB as f64,
        seconds,
        mb_per_s: mb_per_s(FLOOD_MB as f64, seconds),
        frames: stats,
    })
}

/// B3 for one renderer.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let mut run = launch_ready(o, renderer, None)?;
    let tab = run.tab.clone();
    let dir = run.ctx.ensure_world().root.clone();
    let r = run_flood(&mut run.app, &tab, &dir)?;
    pause(200);
    run.app.close();
    Ok(vec![Measure {
        id: "B3",
        renderer,
        value: r.mb_per_s,
        unit: "MB/s",
        detail: json!({
            "mb": r.mb,
            "seconds": r.seconds,
            "short": r.seconds < (FRAMES_START_MS as f64 / 1000.0) + 0.05,
            "frames": r.frames.to_json(),
        }),
    }])
}
