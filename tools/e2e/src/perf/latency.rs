//! B4, keystroke latency (Task 60, D5): the time from a key entering the app's key path to the
//! presented frame that holds the echoed character, measured inside the app (`Latency`).
//!
//! 200 keys, 20 ms apart, at the `cmd` prompt. The expected text is the whole line typed so far
//! (up to 20 characters, then Escape clears the line), so each wait names text that is not on the
//! screen yet: a letter that was echoed before can never complete a later wait early.

use std::time::{Duration, Instant};

use flare_e2e_proto::{Reply, Request};
use serde_json::json;

use super::{launch_ready, median, pause, percentile, Measure, PerfOpts};
use crate::ctx::App;

/// How many keys the measure types.
pub const KEYS: usize = 200;
/// The gap between two keys.
pub const GAP_MS: u64 = 20;
/// Characters on the line before Escape clears it.
pub const LINE_LEN: usize = 20;
/// The longest a single key may take to appear.
const KEY_TIMEOUT_MS: u64 = 5_000;

/// The first keys of every line are typed without being timed: a one or two letter wait could be
/// satisfied by letters already on the screen (the prompt's path), a three letter one cannot.
pub const WARM_KEYS: usize = 2;

/// One key of the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The letter pressed.
    pub letter: char,
    /// The whole line after the key: what the frame must show.
    pub expect: String,
    /// Press Escape first: the line starts empty.
    pub clear_first: bool,
    /// False for the first keys of a line, which are typed but not timed.
    pub measured: bool,
}

/// The keys that give `n` timed ones, letters stepping through a to z with a different start for
/// every line.
pub fn plan(n: usize) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut timed = 0;
    let mut line = String::new();
    let mut i = 0;
    while timed < n {
        let pos = i % LINE_LEN;
        let row = i / LINE_LEN;
        let clear_first = pos == 0 && i > 0;
        if pos == 0 {
            line.clear();
        }
        let letter = (b'a' + ((row * 5 + pos * 7) % 26) as u8) as char;
        line.push(letter);
        let measured = pos >= WARM_KEYS;
        if measured {
            timed += 1;
        }
        steps.push(Step {
            letter,
            expect: line.clone(),
            clear_first,
            measured,
        });
        i += 1;
    }
    steps
}

/// The result of the series.
#[derive(Clone, Debug, PartialEq)]
pub struct LatencyResult {
    pub n: usize,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// Summarises the per-key times.
pub fn summarise(times: &[f64]) -> LatencyResult {
    LatencyResult {
        n: times.len(),
        median_ms: median(times),
        p95_ms: percentile(times, 95.0),
        max_ms: times.iter().copied().fold(0.0, f64::max),
    }
}

/// Types `steps` into the front tab, one key every `GAP_MS` on a fixed schedule, and returns the
/// app's own key-to-frame time for each.
pub fn series(app: &mut App, steps: &[Step]) -> Result<Vec<f64>, String> {
    let start = Instant::now();
    let mut times = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        if s.clear_first {
            app.session.key("Escape")?;
        }
        let due = start + Duration::from_millis(GAP_MS * i as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            pause(wait.as_millis() as u64);
        }
        let chord = s.letter.to_string();
        if !s.measured {
            app.session.key(&chord)?;
            continue;
        }
        match app.session.call(&Request::Latency {
            chord: chord.clone(),
            expect: s.expect.clone(),
            // Latency reads the front tab only; the only tab is the front one.
            tab: None,
            timeout_ms: KEY_TIMEOUT_MS,
        })? {
            Reply::Latency { ms } => times.push(ms),
            other => return Err(format!("expected Latency, got {other:?}")),
        }
        app.session.call(&Request::Key { chord, down: false })?;
    }
    Ok(times)
}

/// B4 for one renderer.
pub fn latency(o: &PerfOpts, renderer: &'static str) -> Result<LatencyResult, String> {
    let mut run = launch_ready(o, renderer, None)?;
    let times = series(&mut run.app, &plan(KEYS))?;
    run.app.close();
    Ok(summarise(&times))
}

/// B4 as a measure.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let r = latency(o, renderer)?;
    Ok(vec![Measure {
        id: "B4",
        renderer,
        value: r.median_ms,
        unit: "ms",
        detail: json!({ "n": r.n, "p95_ms": r.p95_ms, "max_ms": r.max_ms }),
    }])
}
