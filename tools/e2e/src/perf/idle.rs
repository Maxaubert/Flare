//! B6, idle CPU (Task 60, D5): the processor time (kernel plus user, 100 ns units from
//! `GetProcessTimes`) of the app's process tree, minus the shells and what they start, over 10 s at
//! a prompt with one tab and no agent. Reported as a percentage of one core.
//!
//! The 3 s before the first reading let the work that follows the first frame finish: that is
//! start-up cost (B1), not idle cost.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::json;

use super::{
    launch_ready, pause, process_cpu_100ns, process_table, tree_cpu, Measure, PerfOpts, ProcEntry,
};

/// How long the app is left alone after its prompt before the first reading.
pub const WARMUP_MS: u64 = 3_000;
/// The length of the measured window.
pub const WINDOW_MS: u64 = 10_000;

/// The CPU time the processes present at both readings used in between, in 100 ns units. A
/// process that started or ended in between is not counted: its time is not a delta.
pub fn cpu_delta(before: &HashMap<u32, u64>, after: &HashMap<u32, u64>) -> u64 {
    before
        .iter()
        .filter_map(|(pid, b)| after.get(pid).map(|a| a.saturating_sub(*b)))
        .sum()
}

/// Percent of one core that `delta_100ns` of CPU time is over `wall_ms` of wall time.
pub fn pct_of_core(delta_100ns: u64, wall_ms: f64) -> f64 {
    if wall_ms <= 0.0 {
        return 0.0;
    }
    (delta_100ns as f64 / 10_000.0) / wall_ms * 100.0
}

/// One reading of the app tree's CPU times.
pub fn reading(table: &[ProcEntry], root: u32) -> HashMap<u32, u64> {
    tree_cpu(table, root, &mut |pid| process_cpu_100ns(pid))
}

/// B6 for one renderer.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let mut run = launch_ready(o, renderer, None)?;
    let root = run.app.child.pid();
    pause(WARMUP_MS);
    let table = process_table();
    let before = reading(&table, root);
    let t0 = Instant::now();
    pause(WINDOW_MS);
    let after = reading(&process_table(), root);
    let wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let delta = cpu_delta(&before, &after);
    run.app.close();
    Ok(vec![Measure {
        id: "B6",
        renderer,
        value: pct_of_core(delta, wall_ms),
        unit: "% of a core",
        detail: json!({
            "wall_ms": wall_ms,
            "cpu_ms": delta as f64 / 10_000.0,
            "processes": before.len(),
        }),
    }])
}
