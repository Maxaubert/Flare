//! B1, cold start to a usable prompt (Task 60, D5).
//!
//! `usable_ms` is measured inside the app on the clock that starts at process creation (the
//! loader's time counts): the end of the first frame showing the shell's prompt, plus the time to
//! the first frame that shows one echoed key. The harness queues that key BEFORE the prompt exists
//! (`KeyAtPrompt`), so its own polling never enters the number. It reads both values back from the
//! `startup.trace` probe. The wall clock from `CreateProcess` to the probe answering is reported
//! beside it as a cross-check, not gated. Median of N launches after one discarded launch, each in
//! a fresh world with `--no-import` (no first-run import in the number).

use std::time::{Duration, Instant};

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::{median, seed_profile, Measure, PerfOpts, NO_GPU};
use crate::ctx::{park_problems, Ctx};
use crate::proc::Child;
use crate::session::Session;
use crate::until::{wait_on, SystemClock};

/// How long a launch waits for `usable_ms` to appear in the trace.
const USABLE_WAIT_MS: u64 = 10_000;
/// How long a launch waits for the app's pipe.
const CONNECT_WAIT: Duration = Duration::from_secs(20);

/// One launch.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Process creation to the end of the first frame holding the prompt.
    pub prompt_ms: f64,
    /// Process creation to the end of the first frame holding the echoed key.
    pub usable_ms: f64,
    /// The harness clock, `CreateProcess` to the trace answering (a cross-check).
    pub outside_ms: f64,
    /// The app's `startup.trace` phases.
    pub phases: Vec<(String, f64)>,
}

impl Sample {
    /// The echo's own time: a prompt that does not take input is not usable.
    pub fn echo_ms(&self) -> f64 {
        self.usable_ms - self.prompt_ms
    }
}

/// The result of all launches.
#[derive(Clone, Debug, PartialEq)]
pub struct StartupResult {
    pub runs_ms: Vec<f64>,
    pub median_ms: f64,
    pub prompt_median_ms: f64,
    pub echo_median_ms: f64,
    pub outside_median_ms: f64,
    /// The `startup.trace` of the median run.
    pub phases: Vec<(String, f64)>,
}

/// Reads a sample out of the `startup.trace` probe's answer. The run value is the app's own
/// `usable_ms`; `outside_ms` is only carried along. None until both marks are known.
pub fn sample_from_trace(trace: &Value, outside_ms: f64) -> Option<Sample> {
    let prompt_ms = trace.get("prompt_ms")?.as_f64()?;
    let usable_ms = trace.get("usable_ms")?.as_f64()?;
    let phases = trace
        .get("phases")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    Some((p.get("name")?.as_str()?.to_string(), p.get("ms")?.as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Sample {
        prompt_ms,
        usable_ms,
        outside_ms,
        phases,
    })
}

/// Runs `one` once and throws that launch away (the disk cache and the first-use costs are not
/// the start-up), then `runs` more times and takes the medians.
pub fn collect(
    runs: usize,
    one: &mut dyn FnMut() -> Result<Sample, String>,
) -> Result<StartupResult, String> {
    one()?;
    let mut samples = Vec::new();
    for _ in 0..runs.max(1) {
        samples.push(one()?);
    }
    let usable: Vec<f64> = samples.iter().map(|s| s.usable_ms).collect();
    let med = median(&usable);
    // The run whose value is nearest the median shows the phases.
    let nearest = samples
        .iter()
        .min_by(|a, b| {
            (a.usable_ms - med)
                .abs()
                .total_cmp(&(b.usable_ms - med).abs())
        })
        .expect("at least one run");
    let prompts: Vec<f64> = samples.iter().map(|s| s.prompt_ms).collect();
    let echoes: Vec<f64> = samples.iter().map(Sample::echo_ms).collect();
    let outside: Vec<f64> = samples.iter().map(|s| s.outside_ms).collect();
    Ok(StartupResult {
        runs_ms: usable,
        median_ms: med,
        prompt_median_ms: median(&prompts),
        echo_median_ms: median(&echoes),
        outside_median_ms: median(&outside),
        phases: nearest.phases.clone(),
    })
}

/// One real launch: a fresh world, the key queued at once, the trace read back.
fn launch_once(o: &PerfOpts, renderer: &'static str) -> Result<Sample, String> {
    let mut ctx = Ctx::new("perf-startup");
    ctx.exe = o.exe.clone();
    let (profile, alpha, env) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.alpha.clone(), w.env())
    };
    seed_profile(&profile, o.shell)?;
    let alpha = alpha.to_string_lossy().into_owned();
    let args = ["--no-import", "--renderer", renderer, alpha.as_str()];
    let began = Instant::now();
    let child = Child::launch(&o.exe, &args, &env, &profile);
    let mut session = Session::connect(&profile, CONNECT_WAIT)?;
    // The very first request: the key waits for the prompt, so no polling of ours is in the number.
    session.call(&Request::KeyAtPrompt {
        chord: "x".to_string(),
        prompt_suffix: ">".to_string(),
    })?;
    let mut sample = None;
    let mut last = String::from("startup.trace never answered");
    let held = wait_on(
        &mut SystemClock::new(),
        USABLE_WAIT_MS,
        &mut || match session.probe_str("startup.trace") {
            Ok(v) => match sample_from_trace(&v, began.elapsed().as_secs_f64() * 1000.0) {
                Some(s) => {
                    sample = Some(s);
                    true
                }
                None => {
                    last = format!("usable_ms is not known yet: {v}");
                    false
                }
            },
            Err(e) => {
                last = e;
                false
            }
        },
    );
    if !held {
        return Err(format!("the prompt never became usable ({last})"));
    }
    if renderer == "gpu" {
        let hello = session.hello()?;
        if !hello.renderer.to_lowercase().starts_with("gpu") {
            return Err(format!("{NO_GPU} ({})", hello.renderer));
        }
    }
    // The safety line, after the number is taken: the window was off every monitor.
    let problems = park_problems(&session.probe_str("window.park")?);
    if !problems.is_empty() {
        return Err(format!(
            "the --e2e window is not safe: {}",
            problems.join("; ")
        ));
    }
    let _ = session.call(&Request::Quit { force: true });
    child.wait_for(|| child.exited().is_some(), Duration::from_secs(10));
    sample.ok_or_else(|| "no sample".to_string())
}

/// B1 for one renderer: the launches, discarding the first.
pub fn startup(o: &PerfOpts, renderer: &'static str) -> Result<StartupResult, String> {
    collect(o.runs, &mut || launch_once(o, renderer))
}

/// B1 as a measure.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let r = startup(o, renderer)?;
    Ok(vec![Measure {
        id: "B1",
        renderer,
        value: r.median_ms,
        unit: "ms",
        detail: json!({
            "shell": o.shell,
            "runs_ms": r.runs_ms,
            "prompt_median_ms": r.prompt_median_ms,
            "echo_median_ms": r.echo_median_ms,
            "outside_median_ms": r.outside_median_ms,
            "phases": r.phases.iter().map(|(n, ms)| json!({"name": n, "ms": ms})).collect::<Vec<_>>(),
        }),
    }])
}
