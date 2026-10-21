//! The performance harness (M10 Task 60): the bar, the start-up statistics, the process-tree rules,
//! the plans of the latency, flood and scroll measures, and the report. Nothing here starts an app.

use std::collections::HashMap;

use flare_e2e::perf::bar::{bar, evaluate, parse_rule, pt_for, Verdict};
use flare_e2e::perf::startup::{collect, sample_from_trace, Sample};
use flare_e2e::perf::{
    app_tree, civil_date, expand, is_shell_name, judge, median, percentile, report_json,
    table_text, tree_cpu, Measure, PerfOpts, ProcEntry, IDS,
};
use flare_e2e::until::Clock;
use serde_json::{json, Value};

/// (id, renderer, value, detail, pt, expected)
type EdgeCase<'a> = (&'a str, &'a str, f64, Value, Option<f64>, Verdict);

fn ev(id: &str, renderer: &str, value: f64, detail: Value, pt: Option<f64>) -> Verdict {
    evaluate(&bar(), id, renderer, value, &detail, pt).0
}

// ---- the bar ----------------------------------------------------------------------------------------

#[test]
fn the_bar_evaluator_passes_and_fails_each_rule_at_its_edge() {
    use Verdict::{Fail, Pass};
    let none = json!({});
    // (id, renderer, value, detail, pt, expected)
    let table: Vec<EdgeCase> = vec![
        // B1: at most 300 ms and at most Prism Terminal's.
        ("B1", "gpu", 300.0, none.clone(), Some(300.0), Pass),
        ("B1", "gpu", 300.001, none.clone(), Some(900.0), Fail),
        ("B1", "gpu", 250.0, none.clone(), Some(249.9), Fail),
        ("B1", "software", 200.0, none.clone(), Some(200.0), Pass),
        ("B1", "software", 301.0, none.clone(), Some(900.0), Fail),
        // B2: gpu at most a third of Prism Terminal's, software at most equal.
        ("B2", "gpu", 100.0, none.clone(), Some(300.0), Pass),
        ("B2", "gpu", 100.01, none.clone(), Some(300.0), Fail),
        ("B2", "software", 300.0, none.clone(), Some(300.0), Pass),
        ("B2", "software", 300.01, none.clone(), Some(300.0), Fail),
        // B3: gpu at least twice, software at least equal; no gpu frame over 33 ms.
        (
            "B3",
            "gpu",
            200.0,
            json!({"frames": {"max_ms": 33.0}}),
            Some(100.0),
            Pass,
        ),
        (
            "B3",
            "gpu",
            200.0,
            json!({"frames": {"max_ms": 33.1}}),
            Some(100.0),
            Fail,
        ),
        (
            "B3",
            "gpu",
            199.9,
            json!({"frames": {"max_ms": 10.0}}),
            Some(100.0),
            Fail,
        ),
        (
            "B3",
            "software",
            100.0,
            json!({"frames": {"max_ms": 900.0}}),
            Some(100.0),
            Pass,
        ),
        (
            "B3",
            "software",
            99.9,
            json!({"frames": {"max_ms": 1.0}}),
            Some(100.0),
            Fail,
        ),
        // B4: median at most Prism Terminal's, p95 at most 25 ms.
        ("B4", "gpu", 5.0, json!({"p95_ms": 25.0}), Some(5.0), Pass),
        ("B4", "gpu", 5.0, json!({"p95_ms": 25.1}), Some(5.0), Fail),
        ("B4", "gpu", 5.1, json!({"p95_ms": 10.0}), Some(5.0), Fail),
        // B5: 55 fps, and no gpu frame over 33 ms.
        ("B5", "gpu", 55.0, json!({"max_ms": 33.0}), None, Pass),
        ("B5", "gpu", 54.9, json!({"max_ms": 10.0}), None, Fail),
        ("B5", "gpu", 60.0, json!({"max_ms": 33.1}), None, Fail),
        ("B5", "software", 60.0, json!({"max_ms": 500.0}), None, Pass),
        // B6: at most half a percent of a core and at most Prism Terminal's.
        ("B6", "gpu", 0.5, none.clone(), Some(0.5), Pass),
        ("B6", "gpu", 0.51, none.clone(), Some(5.0), Fail),
        ("B6", "gpu", 0.4, none.clone(), Some(0.39), Fail),
        // B7: render p95 at most 8.3 ms and no interval over 33 ms.
        ("B7", "gpu", 8.3, json!({"max_ms": 33.0}), None, Pass),
        ("B7", "gpu", 8.31, json!({"max_ms": 10.0}), None, Fail),
        ("B7", "gpu", 4.0, json!({"max_ms": 33.1}), None, Fail),
    ];
    for (id, renderer, value, detail, pt, want) in table {
        assert_eq!(
            ev(id, renderer, value, detail.clone(), pt),
            want,
            "{id} {renderer} value {value} pt {pt:?} detail {detail}"
        );
    }
}

#[test]
fn a_missing_baseline_makes_pt_relative_rules_unknown_not_pass() {
    use Verdict::{Fail, Unknown};
    let none = json!({});
    // Inside the absolute limit, but the Prism Terminal rule cannot be judged.
    assert_eq!(ev("B1", "gpu", 250.0, none.clone(), None), Unknown);
    assert_eq!(ev("B2", "gpu", 10.0, none.clone(), None), Unknown);
    assert_eq!(
        ev("B3", "gpu", 500.0, json!({"frames": {"max_ms": 5.0}}), None),
        Unknown
    );
    assert_eq!(ev("B4", "gpu", 1.0, json!({"p95_ms": 2.0}), None), Unknown);
    assert_eq!(ev("B6", "gpu", 0.1, none.clone(), None), Unknown);
    // A number that already breaks its absolute limit fails without a baseline.
    assert_eq!(ev("B1", "gpu", 301.0, none.clone(), None), Fail);
    assert_eq!(ev("B6", "gpu", 0.6, none.clone(), None), Fail);
    assert_eq!(
        ev(
            "B3",
            "gpu",
            500.0,
            json!({"frames": {"max_ms": 40.0}}),
            None
        ),
        Fail
    );
    // A detail that is not there is unknown, never a pass.
    assert_eq!(ev("B3", "gpu", 500.0, none.clone(), Some(100.0)), Unknown);
    assert_eq!(ev("B7", "gpu", 1.0, none, None), Unknown);
}

#[test]
fn rules_are_read_as_written() {
    let r = parse_rule("<= pt / 3").unwrap();
    assert_eq!(r.limit(300.0), 100.0);
    assert!(r.holds(100.0, 300.0) && !r.holds(100.01, 300.0));
    let r = parse_rule(">= pt * 2").unwrap();
    assert!(r.holds(200.0, 100.0) && !r.holds(199.99, 100.0));
    let r = parse_rule("<= pt").unwrap();
    assert!(r.holds(5.0, 5.0));
    assert!(parse_rule("== pt").is_err());
    assert!(parse_rule("<= 3").is_err());
    assert!(parse_rule("<= pt / 0").is_err());
}

#[test]
fn the_baseline_numbers_come_from_the_pt_perf_shape() {
    let b = json!({
        "start_ms": { "cmd": { "median": 900.0 }, "pwsh": { "median": 1500.0 } },
        "idle_private_ws_mb": { "cmd": { "total": 600.0 } },
        "flood": { "mb_per_s": 40.0 },
        "key_latency_ms": { "median": 12.0 },
        "scroll": { "fps": 58.0 },
        "idle_cpu_pct_of_core": { "cmd": 1.5 },
    });
    assert_eq!(pt_for("B1", "cmd", &b), Some(900.0));
    assert_eq!(pt_for("B1", "pwsh", &b), Some(1500.0));
    assert_eq!(pt_for("B2", "cmd", &b), Some(600.0));
    assert_eq!(pt_for("B3", "cmd", &b), Some(40.0));
    assert_eq!(pt_for("B4", "cmd", &b), Some(12.0));
    assert_eq!(pt_for("B5", "cmd", &b), Some(58.0));
    assert_eq!(pt_for("B6", "cmd", &b), Some(1.5));
    assert_eq!(pt_for("B7", "cmd", &b), None);
    assert_eq!(pt_for("B2", "cmd", &json!({})), None);
}

// ---- start-up ---------------------------------------------------------------------------------------

fn sample(prompt: f64, usable: f64, outside: f64) -> Sample {
    Sample {
        prompt_ms: prompt,
        usable_ms: usable,
        outside_ms: outside,
        phases: vec![("main".to_string(), usable / 10.0)],
    }
}

#[test]
fn startup_median_is_taken_after_one_discarded_launch() {
    // The first launch is slow (cold disk cache) and must not count.
    let mut launches = vec![
        sample(2000.0, 2100.0, 2200.0),
        sample(100.0, 130.0, 150.0),
        sample(120.0, 160.0, 180.0),
        sample(110.0, 140.0, 160.0),
    ]
    .into_iter();
    let mut calls = 0;
    let r = collect(3, &mut || {
        calls += 1;
        launches.next().ok_or("too many launches".to_string())
    })
    .unwrap();
    assert_eq!(calls, 4, "one discarded launch and three counted");
    assert_eq!(r.runs_ms, vec![130.0, 160.0, 140.0]);
    assert_eq!(r.median_ms, 140.0);
    assert_eq!(r.prompt_median_ms, 110.0);
    assert_eq!(r.echo_median_ms, 30.0);
    assert_eq!(r.outside_median_ms, 160.0);
    assert_eq!(
        r.phases,
        vec![("main".to_string(), 14.0)],
        "the median run's trace"
    );
}

#[test]
fn a_failed_launch_ends_the_series() {
    let mut n = 0;
    let r = collect(3, &mut || {
        n += 1;
        if n == 3 {
            Err("the prompt never became usable".to_string())
        } else {
            Ok(sample(1.0, 2.0, 3.0))
        }
    });
    assert_eq!(r.unwrap_err(), "the prompt never became usable");
}

#[test]
fn startup_uses_the_in_app_usable_time_not_the_harness_clock() {
    let trace = json!({
        "prompt_ms": 211.5,
        "usable_ms": 243.25,
        "phases": [ {"name": "main", "ms": 12.0}, {"name": "first_frame", "ms": 200.0} ],
    });
    // The harness saw the answer 9000 ms after it started the process (a slow pipe, a busy PC).
    let s = sample_from_trace(&trace, 9000.0).expect("both marks are known");
    assert_eq!(
        s.usable_ms, 243.25,
        "the run value is the app's own usable_ms"
    );
    assert_eq!(s.prompt_ms, 211.5);
    assert_eq!(
        s.outside_ms, 9000.0,
        "the outside time is only carried along"
    );
    assert_eq!(s.phases.len(), 2);
    assert_eq!(s.phases[1], ("first_frame".to_string(), 200.0));
    // Until the app knows both numbers there is no sample.
    assert!(sample_from_trace(&json!({"prompt_ms": 5.0, "usable_ms": null}), 1.0).is_none());
    assert!(sample_from_trace(&json!({"prompt_ms": null, "usable_ms": null}), 1.0).is_none());
    assert!(sample_from_trace(&json!({}), 1.0).is_none());
}

// ---- processes ---------------------------------------------------------------------------------------

fn p(pid: u32, ppid: u32, name: &str) -> ProcEntry {
    ProcEntry {
        pid,
        ppid,
        name: name.to_string(),
    }
}

fn table() -> Vec<ProcEntry> {
    vec![
        p(1, 0, "explorer.exe"),
        p(10, 1, "flare.exe"),
        p(11, 10, "OpenConsole.exe"),
        p(12, 11, "cmd.exe"),
        p(13, 12, "node.exe"),
        p(14, 13, "helper-of-an-agent.exe"),
        p(15, 10, "flare-gpu.exe"),
        p(16, 15, "conhost.exe"),
        p(17, 10, "PWSH.EXE"),
        p(18, 17, "tool.exe"),
        p(99, 1, "other.exe"),
    ]
}

#[test]
fn the_tree_walk_excludes_shells_and_their_children() {
    let mut tree = app_tree(&table(), 10);
    tree.sort_unstable();
    // The app and its own helper stay; every shell, its host and everything they started go.
    assert_eq!(tree, vec![10, 15]);
    // The root is kept even if it is named like a shell.
    assert_eq!(
        app_tree(&[p(5, 0, "cmd.exe"), p(6, 5, "x.exe")], 5),
        vec![5, 6]
    );
    assert!(is_shell_name("cmd") && is_shell_name("CMD.EXE") && is_shell_name("OpenConsole.exe"));
    assert!(
        is_shell_name("conhost.exe") && is_shell_name("node") && is_shell_name("powershell.exe")
    );
    assert!(!is_shell_name("flare.exe") && !is_shell_name("cmdline-tool.exe"));
}

#[test]
fn idle_cpu_counts_only_the_app_tree() {
    use flare_e2e::perf::idle::{cpu_delta, pct_of_core};
    let cpu: HashMap<u32, u64> = [(10, 50_000), (11, 7_000_000), (12, 9_000_000), (15, 20_000)]
        .into_iter()
        .collect();
    let mut asked = Vec::new();
    let before = tree_cpu(&table(), 10, &mut |pid| {
        asked.push(pid);
        cpu.get(&pid).copied()
    });
    asked.sort_unstable();
    assert_eq!(asked, vec![10, 15], "the shells' times are never even read");
    assert_eq!(before.len(), 2);
    // Ten seconds later the app used 30 ms and its helper 20 ms; a process that appeared in
    // between has no delta and is not counted.
    let after: HashMap<u32, u64> = [(10, 350_000), (15, 220_000), (77, 99_000_000)]
        .into_iter()
        .collect();
    let delta = cpu_delta(&before, &after);
    assert_eq!(delta, 300_000 + 200_000, "50 ms in 100 ns units");
    let pct = pct_of_core(delta, 10_000.0);
    assert!(
        (pct - 0.5).abs() < 1e-9,
        "50 ms over 10 s is 0.5% of a core, got {pct}"
    );
    assert_eq!(pct_of_core(1, 0.0), 0.0);
}

#[test]
fn private_ws_uses_the_same_query_as_the_pt_tool() {
    use flare_e2e::perf::memory::{private_ws_script, PERF_CLASS, PERF_PROPERTY};
    let pt = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../pt-perf/pt-perf.mjs"
    ))
    .expect("tools/pt-perf/pt-perf.mjs");
    let script = private_ws_script(&[10, 15]);
    assert_eq!(PERF_CLASS, "Win32_PerfFormattedData_PerfProc_Process");
    assert_eq!(PERF_PROPERTY, "WorkingSetPrivate");
    for needle in [PERF_CLASS, PERF_PROPERTY] {
        assert!(pt.contains(needle), "pt-perf.mjs lacks {needle}");
        assert!(script.contains(needle), "the Flare script lacks {needle}");
    }
    assert!(
        script.contains("@(10,15)"),
        "the pids are in the script: {script}"
    );
}

#[test]
fn the_working_set_answer_is_read_for_one_process_or_many() {
    use flare_e2e::perf::memory::{parse_private_ws, total_mb};
    assert_eq!(
        parse_private_ws(r#"{"pid":4,"bytes":1048576.0}"#),
        vec![(4, 1_048_576.0)]
    );
    let many = parse_private_ws(r#"[{"pid":4,"bytes":1048576.0},{"pid":8,"bytes":3145728.0}]"#);
    assert_eq!(many.len(), 2);
    assert_eq!(total_mb(&many), 4.0);
    assert!(parse_private_ws("").is_empty());
    assert!(parse_private_ws("not json").is_empty());
}

// ---- latency, flood, scroll, frames ------------------------------------------------------------------

#[test]
fn the_latency_plan_never_waits_on_text_that_is_already_there() {
    use flare_e2e::perf::latency::{plan, LINE_LEN, WARM_KEYS};
    let steps = plan(200);
    let timed: Vec<_> = steps.iter().filter(|s| s.measured).collect();
    assert_eq!(timed.len(), 200);
    let mut line = String::new();
    for s in &steps {
        if s.clear_first {
            line.clear();
        }
        line.push(s.letter);
        assert_eq!(s.expect, line, "the wait names the whole line typed so far");
        if s.measured {
            assert!(
                s.expect.chars().count() > WARM_KEYS,
                "a timed wait names at least three characters"
            );
        }
    }
    assert!(!steps[0].clear_first && steps[LINE_LEN].clear_first);
    assert!(steps.iter().all(|s| s.letter.is_ascii_lowercase()));
}

#[test]
fn latency_is_summarised_by_nearest_rank() {
    use flare_e2e::perf::latency::summarise;
    let times: Vec<f64> = (1..=100).map(f64::from).collect();
    let r = summarise(&times);
    assert_eq!(
        (r.n, r.median_ms, r.p95_ms, r.max_ms),
        (100, 50.5, 95.0, 100.0)
    );
    assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    assert_eq!(percentile(&[], 95.0), 0.0);
}

struct FakeClock {
    now: u64,
    slept: Vec<u64>,
}

impl Clock for FakeClock {
    fn elapsed_ms(&self) -> u64 {
        self.now
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.slept.push(ms);
        self.now += ms;
    }
}

#[test]
fn frames_during_the_flood_start_500_ms_after_enter() {
    use flare_e2e::perf::flood::{wait_frames_start, FRAMES_START_MS};
    assert_eq!(FRAMES_START_MS, 500);
    // The Enter took 7 ms to be written: the wait is the rest of the 500 ms, no more.
    let mut c = FakeClock {
        now: 7,
        slept: Vec::new(),
    };
    wait_frames_start(&mut c);
    assert_eq!(c.now, 500);
    assert_eq!(c.slept, vec![493]);
    // Already past: no wait.
    let mut late = FakeClock {
        now: 800,
        slept: Vec::new(),
    };
    wait_frames_start(&mut late);
    assert!(late.slept.is_empty());
}

#[test]
fn the_flood_file_is_100_mb_and_ends_with_the_marker() {
    use flare_e2e::perf::flood::{ensure_flood_file, mb_per_s, FLOOD_END, FLOOD_MB};
    use std::io::{Read, Seek, SeekFrom};
    assert_eq!(FLOOD_MB, 100);
    let dir = tempfile::tempdir().unwrap();
    let path = ensure_flood_file(dir.path());
    let mut f = std::fs::File::open(&path).unwrap();
    assert_eq!(f.metadata().unwrap().len(), 100 * 1024 * 1024);
    f.seek(SeekFrom::End(-((FLOOD_END.len() + 1) as i64)))
        .unwrap();
    let mut tail = String::new();
    f.read_to_string(&mut tail).unwrap();
    assert_eq!(tail, format!("{FLOOD_END}\n"));
    // A second call reuses the file.
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(ensure_flood_file(dir.path()), path);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    assert_eq!(mb_per_s(100.0, 2.0), 50.0);
    assert_eq!(mb_per_s(100.0, 0.0), 0.0);
}

#[test]
fn the_scroll_file_has_100000_lines_ending_with_the_marker() {
    use flare_e2e::perf::scroll::{fps, scroll_file, LINES, SCROLL_END, WHEEL_EVENTS};
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(scroll_file(dir.path())).unwrap();
    assert_eq!(text.lines().count(), LINES);
    assert_eq!(text.lines().last(), Some(SCROLL_END));
    assert_eq!(WHEEL_EVENTS, 5000);
    assert_eq!(fps(300, 5.0), 60.0);
}

#[test]
fn the_resize_drag_goes_down_and_back_for_three_seconds() {
    use flare_e2e::perf::frames::{drag_sizes, DRAG_MIN};
    let sizes = drag_sizes((1000.0, 640.0));
    assert_eq!(sizes.len(), 3000 / 16);
    assert_eq!(sizes[0], (1000.0, 640.0));
    assert_eq!(*sizes.last().unwrap(), (1000.0, 640.0));
    let min_w = sizes.iter().map(|s| s.0).fold(f32::MAX, f32::min);
    assert!(
        (min_w - DRAG_MIN.0).abs() < 2.0,
        "the drag reaches the small size, got {min_w}"
    );
    assert!(sizes
        .iter()
        .all(|s| s.0 >= DRAG_MIN.0 - 0.01 && s.0 <= 1000.01));
}

// ---- the report ---------------------------------------------------------------------------------------

fn full_set() -> Vec<Measure> {
    let mut out = Vec::new();
    for (i, id) in IDS.iter().enumerate() {
        for renderer in ["gpu", "software"] {
            out.push(Measure {
                id,
                renderer,
                value: 10.0 + i as f64,
                unit: "ms",
                detail: json!({"max_ms": 1.0, "p95_ms": 1.0, "frames": {"max_ms": 1.0}}),
            });
        }
    }
    out
}

#[test]
fn the_report_json_has_every_id_and_renderer() {
    let rows = judge(&full_set(), None, "cmd");
    let opts = PerfOpts::new("flare.exe".into());
    let report = report_json(&rows, &opts, "2026-10-19");
    for id in IDS {
        for renderer in ["gpu", "software"] {
            assert!(
                report["values"][id][renderer].is_number(),
                "values.{id}.{renderer} is missing"
            );
        }
    }
    assert_eq!(report["measures"].as_array().unwrap().len(), 14);
    assert_eq!(report["taken"], "2026-10-19");
    assert_eq!(
        report["measures"][0]["verdict"], "unknown",
        "no baseline, no pass"
    );
    let table = table_text(&rows);
    assert!(table.starts_with("id"));
    assert_eq!(table.lines().count(), 15, "a header and 14 rows");
    assert!(table.contains("unknown"));
}

#[test]
fn a_pwsh_start_is_reported_with_no_bar() {
    let m = vec![Measure {
        id: "B1",
        renderer: "gpu",
        value: 5000.0,
        unit: "ms",
        detail: json!({}),
    }];
    let rows = judge(&m, None, "pwsh");
    assert_eq!(rows[0].verdict, Verdict::Unknown);
    assert_eq!(rows[0].rule, "reported, no bar");
    assert_eq!(judge(&m, None, "cmd")[0].verdict, Verdict::Fail);
}

#[test]
fn the_measures_are_named_and_all_means_every_one() {
    assert_eq!(expand(&[]).unwrap().len(), 7);
    assert_eq!(expand(&["all".to_string()]).unwrap().len(), 7);
    assert_eq!(
        expand(&["flood".to_string(), "idle".to_string()]).unwrap(),
        vec!["flood", "idle"]
    );
    assert!(expand(&["nope".to_string()]).is_err());
}

#[test]
fn the_report_date_is_a_calendar_date() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(20_380), "2025-10-19");
    assert_eq!(civil_date(20_746), "2026-10-20");
    assert_eq!(civil_date(11_016), "2000-02-29");
}
