//! The performance harness: the start-up, latency, flood, scroll, idle, memory and frame measures
//! and the bar they are judged against (Task 60, decisions D5 to D7).
//!
//! Every gated number is measured inside the app (the hook's `startup.trace`, `Frames`,
//! `Latency`), so pipe jitter never enters it; the outside wall clock is only a cross-check. One
//! Flare app runs at a time, always `--e2e` (offscreen, unfocusable), each in its own world.

pub mod bar;
pub mod flood;
pub mod frames;
pub mod idle;
pub mod latency;
pub mod memory;
pub mod scroll;
pub mod startup;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::ctx::{App, Ctx, Launch};
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

use bar::Verdict;

/// The measuring ids, in report order.
pub const IDS: [&str; 7] = ["B1", "B2", "B3", "B4", "B5", "B6", "B7"];

/// The error text of a launch that wanted the GPU renderer and did not get it.
pub const NO_GPU: &str = "no GPU adapter";

/// The names `run` accepts (`all` is every one, in this order).
pub const MEASURES: [&str; 7] = [
    "startup", "memory", "flood", "latency", "scroll", "idle", "frames",
];

/// What to measure and how.
#[derive(Clone, Debug)]
pub struct PerfOpts {
    pub exe: PathBuf,
    /// `"gpu"` and/or `"software"`.
    pub renderers: Vec<&'static str>,
    /// Launches that count for the start-up median (one more is discarded).
    pub runs: usize,
    /// The DPI the frame measure runs at: 216 is 225 percent.
    pub scale_dpi: u32,
    /// Where the report JSON goes.
    pub out: PathBuf,
    /// The Prism Terminal baseline (`tools/pt-perf`'s output), when there is one.
    pub baseline: Option<PathBuf>,
    /// The shell of the start-up measure: `"cmd"` (the bar) or `"pwsh"` (reported, no bar).
    pub shell: &'static str,
}

impl PerfOpts {
    pub fn new(exe: PathBuf) -> PerfOpts {
        PerfOpts {
            exe,
            renderers: vec!["gpu", "software"],
            runs: 7,
            scale_dpi: 216,
            out: PathBuf::from("docs/perf/flare-perf.json"),
            baseline: None,
            shell: "cmd",
        }
    }
}

/// One number of the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    /// `"B1"` to `"B7"`.
    pub id: &'static str,
    pub renderer: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub detail: Value,
}

/// The plain name of each bar's number.
pub fn metric_name(id: &str) -> &'static str {
    match id {
        "B1" => "start to usable prompt",
        "B2" => "idle private working set",
        "B3" => "flood throughput",
        "B4" => "key to frame (median)",
        "B5" => "scroll frames per second",
        "B6" => "idle cpu",
        "B7" => "render p95 at 225%",
        _ => "?",
    }
}

// ---- statistics -----------------------------------------------------------------------------------

/// The median; the mean of the two middle values for an even count; 0 for none.
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// The nearest-rank percentile (`p` in 0..=100); 0 for none.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

// ---- processes --------------------------------------------------------------------------------------

/// One row of the process table.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

/// The shells, and what they start, are not the terminal's: `cmd`, `pwsh`, `powershell`,
/// `OpenConsole`, `conhost` and `node` (the agents). Matched by name, with or without `.exe`.
pub fn is_shell_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let n = n.strip_suffix(".exe").unwrap_or(&n);
    [
        "cmd",
        "pwsh",
        "powershell",
        "openconsole",
        "conhost",
        "node",
    ]
    .contains(&n)
}

/// The app's process tree: `root` and every descendant, except a shell-named process and
/// everything below it. The root itself is always kept.
pub fn app_tree(table: &[ProcEntry], root: u32) -> Vec<u32> {
    let mut keep = vec![root];
    let mut i = 0;
    while i < keep.len() {
        let parent = keep[i];
        for p in table {
            if p.ppid == parent
                && p.pid != parent
                && !keep.contains(&p.pid)
                && !is_shell_name(&p.name)
            {
                keep.push(p.pid);
            }
        }
        i += 1;
    }
    keep
}

/// Every process now, with its parent and image name.
pub fn process_table() -> Vec<ProcEntry> {
    let mut out = Vec::new();
    // SAFETY: a plain snapshot of the process list; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: PROCESSENTRY32W is plain data.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: e is sized and snap is valid.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) };
    while ok != 0 {
        let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
        out.push(ProcEntry {
            pid: e.th32ProcessID,
            ppid: e.th32ParentProcessID,
            name: String::from_utf16_lossy(&e.szExeFile[..len]),
        });
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) };
    }
    // SAFETY: closes the snapshot once.
    unsafe {
        CloseHandle(snap);
    }
    out
}

fn filetime(ft: &FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// A process's kernel plus user time in 100 ns units, or None when it cannot be opened.
pub fn process_cpu_100ns(pid: u32) -> Option<u64> {
    // SAFETY: a query-only handle, closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: h is valid and the four out-parameters are live FILETIMEs.
    let ok = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: closes the handle once.
    unsafe {
        CloseHandle(h);
    }
    (ok != 0).then(|| filetime(&kernel) + filetime(&user))
}

// ---- running one app ----------------------------------------------------------------------------------

/// One launched app and the context that owns its world. Dropping it reaps the process and
/// removes the world.
pub struct Running {
    pub ctx: Ctx,
    pub app: App,
    pub tab: String,
}

/// Makes the profile start with one shell and nothing to restore (`settings.json`, no `tabs.json`).
pub fn seed_profile(profile: &Path, shell_id: &str) -> Result<(), String> {
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(profile.join("tabs.json"));
    std::fs::write(
        profile.join("settings.json"),
        json!({ "flare.term.shell": shell_id }).to_string(),
    )
    .map_err(|e| e.to_string())
}

/// Starts the app on `renderer` with one `cmd` tab in a fresh world and waits for its prompt.
/// `NO_GPU` starts the error when the GPU renderer was asked for and not given.
pub fn launch_ready(
    o: &PerfOpts,
    renderer: &'static str,
    scale_dpi: Option<u32>,
) -> Result<Running, String> {
    let mut ctx = Ctx::new("perf");
    ctx.exe = o.exe.clone();
    let (profile, alpha) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.alpha.clone())
    };
    seed_profile(&profile, "cmd")?;
    let mut app = ctx.launch(Launch {
        args: vec!["--no-import".to_string()],
        renderer: Some(renderer),
        folders: vec![alpha],
        scale_dpi,
        ..Launch::default()
    })?;
    if renderer == "gpu" {
        let hello = app.hello()?;
        if !hello.renderer.to_lowercase().starts_with("gpu") {
            return Err(format!("{NO_GPU} ({})", hello.renderer));
        }
    }
    let opened = ctx.poll(15_000, &mut || {
        app.session
            .state()
            .map(|s| s.tabs.len() == 1)
            .unwrap_or(false)
    });
    if !opened {
        return Err("the folder did not open as a tab".into());
    }
    let tab = app
        .session
        .state()?
        .front
        .ok_or("the tab has no id".to_string())?;
    if !wait_for_prompt(&mut app, &tab, Shell::Cmd, prompt_timeout(Shell::Cmd)) {
        return Err("the cmd prompt never showed".into());
    }
    Ok(Running { ctx, app, tab })
}

/// The grid text of the front tab, one row per line (empty when the probe fails).
pub fn grid_text(app: &mut App, tab: &str) -> String {
    crate::world::rows_text(app, tab)
}

/// Sleeps; test code, off the app.
pub fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms)); // sync-ok: test harness, off the app
}

// ---- the report -------------------------------------------------------------------------------------

/// A measure with its verdict, for the table and the JSON.
#[derive(Clone, Debug)]
pub struct Judged {
    pub measure: Measure,
    pub pt: Option<f64>,
    pub rule: String,
    pub verdict: Verdict,
}

/// Judges every measure against the bar and the baseline (when there is one).
pub fn judge(measures: &[Measure], baseline: Option<&Value>, shell: &str) -> Vec<Judged> {
    let rules = bar::bar();
    measures
        .iter()
        .map(|m| {
            let pt = baseline.and_then(|b| bar::pt_for(m.id, shell, b));
            let (mut verdict, mut rule) =
                bar::evaluate(&rules, m.id, m.renderer, m.value, &m.detail, pt);
            if m.id == "B1" && shell == "pwsh" {
                // PowerShell's own start-up (profile, PSReadLine) is not the terminal's: reported.
                verdict = Verdict::Unknown;
                rule = "reported, no bar".to_string();
            }
            Judged {
                measure: m.clone(),
                pt,
                rule,
                verdict,
            }
        })
        .collect()
}

fn fmt_num(v: f64) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// The printed table: `id metric renderer flare rule pt verdict`.
pub fn table_text(rows: &[Judged]) -> String {
    let mut lines = vec![[
        "id".to_string(),
        "metric".into(),
        "renderer".into(),
        "flare".into(),
        "rule".into(),
        "pt".into(),
        "verdict".into(),
    ]];
    for r in rows {
        // A measure that runs in phases and sizes (B7) says which one it is.
        let mut metric = metric_name(r.measure.id).to_string();
        if let Some(phase) = r.measure.detail.get("phase").and_then(Value::as_str) {
            let size = r.measure.detail.get("css_size").and_then(Value::as_array);
            let dims = size.map(|s| {
                let n = |i: usize| s.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                format!(" {}x{}", n(0), n(1))
            });
            metric.push_str(&format!(" ({phase}{})", dims.unwrap_or_default()));
        }
        lines.push([
            r.measure.id.to_string(),
            metric,
            r.measure.renderer.to_string(),
            format!("{} {}", fmt_num(r.measure.value), r.measure.unit),
            r.rule.clone(),
            r.pt.map_or("-".to_string(), fmt_num),
            r.verdict.word().to_string(),
        ]);
    }
    let mut widths = [0usize; 7];
    for l in &lines {
        for (i, c) in l.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut out = String::new();
    for l in &lines {
        let cells: Vec<String> = l
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// The report file: every measure with its detail, rule, Prism Terminal number and verdict, and an
/// index `B1..B7` -> renderer -> value so a later run can be compared at a glance.
pub fn report_json(rows: &[Judged], opts: &PerfOpts, taken: &str) -> Value {
    let mut index = serde_json::Map::new();
    let mut list = Vec::new();
    for r in rows {
        let m = &r.measure;
        let slot = index
            .entry(m.id.to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("the index slot is an object");
        slot.insert(m.renderer.to_string(), json!(m.value));
        list.push(json!({
            "id": m.id,
            "metric": metric_name(m.id),
            "renderer": m.renderer,
            "value": m.value,
            "unit": m.unit,
            "detail": m.detail,
            "rule": r.rule,
            "pt": r.pt,
            "verdict": r.verdict.word(),
        }));
    }
    json!({
        "taken": taken,
        "exe": opts.exe.to_string_lossy(),
        "runs": opts.runs,
        "scale_dpi": opts.scale_dpi,
        "shell": opts.shell,
        "baseline": opts.baseline.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "values": Value::Object(index),
        "measures": list,
    })
}

/// Today as `YYYY-MM-DD` (UTC) from a count of days since 1970-01-01 (Howard Hinnant's civil
/// algorithm), so the report needs no date crate.
pub fn civil_date(days_since_epoch: i64) -> String {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_date(secs.div_euclid(86_400))
}

// ---- running --------------------------------------------------------------------------------------

/// The measures named by the words (`all`, or none, is every one in report order).
pub fn expand(which: &[String]) -> Result<Vec<&'static str>, String> {
    if which.is_empty() || which.iter().any(|w| w == "all") {
        return Ok(MEASURES.to_vec());
    }
    which
        .iter()
        .map(|w| {
            MEASURES
                .iter()
                .find(|m| **m == w.as_str())
                .copied()
                .ok_or_else(|| format!("no measure named {w} (one of {})", MEASURES.join(", ")))
        })
        .collect()
}

fn measure_one(name: &str, o: &PerfOpts, renderer: &'static str) -> Result<Vec<Measure>, String> {
    match name {
        "startup" => startup::measures(o, renderer),
        "memory" => memory::measures(o, renderer),
        "flood" => flood::measures(o, renderer),
        "latency" => latency::measures(o, renderer),
        "scroll" => scroll::measures(o, renderer),
        "idle" => idle::measures(o, renderer),
        "frames" => frames::measures(o, renderer),
        other => Err(format!("no measure named {other}")),
    }
}

/// Runs the named measures on every renderer, one app at a time. A renderer that cannot start
/// (no GPU adapter) is reported and skipped; any other failure ends the run.
pub fn run(which: &[String], opts: &PerfOpts) -> Result<Vec<Measure>, String> {
    let mut all = Vec::new();
    for name in expand(which)? {
        for renderer in &opts.renderers {
            println!("perf {name} on {renderer} ...");
            match measure_one(name, opts, renderer) {
                Ok(ms) => all.extend(ms),
                Err(e) if e.starts_with(NO_GPU) => println!("  skip  {e}"),
                Err(e) => return Err(format!("{name} on {renderer}: {e}")),
            }
        }
    }
    Ok(all)
}

/// The `flare-e2e perf` command: runs, prints the table, writes the report, and returns the exit
/// code (1 when any number breaks the bar; an unknown verdict is not a failure).
pub fn main(cli: &crate::runner::Cli) -> i32 {
    let exe = crate::runner::exe_path(cli);
    if !exe.is_file() {
        println!(
            "{} is missing: run \"cargo build --release -p flare\" first.",
            exe.display()
        );
        return 1;
    }
    let mut opts = PerfOpts::new(exe);
    match cli.renderer.as_deref() {
        None | Some("both") => {}
        Some("gpu") => opts.renderers = vec!["gpu"],
        Some("software") => opts.renderers = vec!["software"],
        Some(other) => {
            println!("--renderer takes gpu, software or both, not {other}");
            return 2;
        }
    }
    match cli.shell.as_deref() {
        None | Some("cmd") => {}
        Some("pwsh") => opts.shell = "pwsh",
        Some(other) => {
            println!("--shell takes cmd or pwsh, not {other}");
            return 2;
        }
    }
    if let Some(n) = cli.runs {
        opts.runs = n.max(1);
    }
    if let Some(pct) = cli.scale {
        opts.scale_dpi = pct * 96 / 100;
    }
    opts.baseline = cli.baseline.clone();
    opts.out = cli
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("docs/perf/flare-{}.json", today())));
    let baseline: Option<Value> = match &opts.baseline {
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => Some(v),
            Err(e) => {
                println!("the baseline {} cannot be read: {e}", p.display());
                return 2;
            }
        },
        None => None,
    };
    let measures = match run(&cli.filters, &opts) {
        Ok(m) => m,
        Err(e) => {
            println!("perf failed: {e}");
            return 1;
        }
    };
    let rows = judge(&measures, baseline.as_ref(), opts.shell);
    println!("\n{}", table_text(&rows));
    let report = report_json(&rows, &opts, &today());
    if let Some(dir) = opts.out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(
        &opts.out,
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    ) {
        Ok(()) => println!("wrote {}", opts.out.display()),
        Err(e) => {
            println!("cannot write {}: {e}", opts.out.display());
            return 1;
        }
    }
    if rows.iter().any(|r| r.verdict == Verdict::Fail) {
        1
    } else {
        0
    }
}

/// Per-pid cpu time of the app tree, for the idle measure and its test.
pub fn tree_cpu(
    table: &[ProcEntry],
    root: u32,
    read: &mut dyn FnMut(u32) -> Option<u64>,
) -> HashMap<u32, u64> {
    app_tree(table, root)
        .into_iter()
        .filter_map(|pid| read(pid).map(|t| (pid, t)))
        .collect()
}
