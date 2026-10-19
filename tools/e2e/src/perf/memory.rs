//! B2, idle memory (Task 60, D5): the private working set (Task Manager's "Memory (private working
//! set)") of the app's process tree, minus the shells and what they start, one tab at the prompt,
//! 10 s after the first prompt, median of three sessions.
//!
//! The number comes from the same PowerShell CIM query `tools/pt-perf` uses on Prism Terminal:
//! `Win32_PerfFormattedData_PerfProc_Process`, property `WorkingSetPrivate`, so the two apps are
//! counted the same way.

use std::process::Command;

use serde_json::{json, Value};

use super::{app_tree, launch_ready, median, pause, process_table, Measure, PerfOpts};

/// The CIM class and property, shared with `tools/pt-perf/pt-perf.mjs` (a test holds them equal).
pub const PERF_CLASS: &str = "Win32_PerfFormattedData_PerfProc_Process";
pub const PERF_PROPERTY: &str = "WorkingSetPrivate";
/// Sessions measured; the median is the number.
pub const SESSIONS: usize = 3;
/// How long after the first prompt the app is read.
pub const SETTLE_MS: u64 = 10_000;

/// The script that prints `[{pid, bytes}]` for the given pids. The pids travel inside the script
/// as integers the harness itself formatted.
pub fn private_ws_script(pids: &[u32]) -> String {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$ids = @({list}); Get-CimInstance {PERF_CLASS} | Where-Object IDProcess -in $ids | \
         ForEach-Object {{ [pscustomobject]@{{ pid = [int]$_.IDProcess; bytes = [double]$_.{PERF_PROPERTY} }} }} | \
         ConvertTo-Json -Compress"
    )
}

/// Reads the script's JSON (an object for one process, an array for several, nothing for none).
pub fn parse_private_ws(text: &str) -> Vec<(u32, f64)> {
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(t) else {
        return Vec::new();
    };
    let rows = match v {
        Value::Array(a) => a,
        one => vec![one],
    };
    rows.iter()
        .filter_map(|r| Some((r.get("pid")?.as_u64()? as u32, r.get("bytes")?.as_f64()?)))
        .collect()
}

/// Runs the query.
fn query(pids: &[u32]) -> Result<Vec<(u32, f64)>, String> {
    let out = Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(private_ws_script(pids))
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "the working-set query failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_private_ws(&String::from_utf8_lossy(&out.stdout)))
}

/// Megabytes of private working set in `rows`.
pub fn total_mb(rows: &[(u32, f64)]) -> f64 {
    rows.iter().map(|(_, b)| b).sum::<f64>() / 1_048_576.0
}

/// B2 for one renderer: the median of three sessions.
pub fn measures(o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    let mut totals = Vec::new();
    let mut last_rows = Vec::new();
    let mut last_breakdown = Value::Null;
    for _ in 0..SESSIONS {
        let mut run = launch_ready(o, renderer, None)?;
        pause(SETTLE_MS);
        let table = process_table();
        let pids = app_tree(&table, run.app.child.pid());
        let rows = query(&pids)?;
        totals.push(total_mb(&rows));
        // Where the app says the bytes are (Task 66): the last session's table goes in the detail.
        last_breakdown = run
            .app
            .session
            .probe("memory.breakdown", json!({}))
            .unwrap_or(Value::Null);
        last_rows = rows
            .iter()
            .map(|(pid, bytes)| {
                let name = table
                    .iter()
                    .find(|p| p.pid == *pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                json!({ "name": name, "pid": pid, "mb": bytes / 1_048_576.0 })
            })
            .collect::<Vec<_>>();
        run.app.close();
    }
    Ok(vec![Measure {
        id: "B2",
        renderer,
        value: median(&totals),
        unit: "MB",
        detail: json!({ "sessions_mb": totals, "processes": last_rows, "breakdown": last_breakdown }),
    }])
}
