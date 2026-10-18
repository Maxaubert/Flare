//! The runner, the context, the one wait primitive and the report (M10 Task 13).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use flare_e2e::ctx::{Ctx, close_child, shot_file};
use flare_e2e::proc::{Child, pid_alive};
use flare_e2e::report::{Row, table};
use flare_e2e::runner::{Outcome, select, supervise};
use flare_e2e::scenarios::ALL;
use flare_e2e::until::{Clock, wait_on};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A clock that never sleeps for real: `sleep_ms` only advances the time it reports.
struct FakeClock {
    now: u64,
    sleeps: u32,
}

impl Clock for FakeClock {
    fn elapsed_ms(&self) -> u64 {
        self.now
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.now += ms;
        self.sleeps += 1;
    }
}

/// A cmd that blocks reading an open stdin pipe and carries `tag` in its command line.
fn blocking(tag: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new("cmd.exe");
    c.raw_arg(format!("/c \"set /p x= & rem {tag}\""))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    c
}

fn profile(tag: &str) -> PathBuf {
    PathBuf::from(format!(
        r"C:\flare-e2e-runner-test\{tag}-{}",
        std::process::id()
    ))
}

#[test]
fn ok_counts_passes_and_failures() {
    let mut ctx = Ctx::new("counting");
    ctx.ok(true, "first");
    ctx.ok(false, "second");
    ctx.ok(true, "third");
    assert_eq!(ctx.checks, 3);
    assert_eq!(ctx.fails, vec!["second".to_string()]);
}

#[test]
fn until_returns_true_as_soon_as_the_condition_holds() {
    let mut clock = FakeClock { now: 0, sleeps: 0 };
    let mut calls = 0;
    let held = wait_on(&mut clock, 1_000, &mut || {
        calls += 1;
        calls == 3
    });
    assert!(held);
    assert_eq!(calls, 3, "checked once per poll, no extra call");
    assert_eq!(clock.sleeps, 2);
    assert_eq!(clock.now, 50, "two polls of 25 ms");
}

#[test]
fn until_records_exactly_one_failure_on_timeout() {
    let mut clock = FakeClock { now: 0, sleeps: 0 };
    let held = wait_on(&mut clock, 100, &mut || false);
    assert!(!held);
    assert!(clock.now >= 100, "gives up only at the timeout");

    // Through the context, a timeout is one counted FAIL with the wait's name.
    let mut ctx = Ctx::new("timeout");
    let held = ctx.until("never", 60, &mut || false);
    assert!(!held);
    assert_eq!(ctx.fails.len(), 1);
    assert_eq!(ctx.fails[0], "timed out waiting for never");
    assert_eq!(ctx.checks, 1);
}

#[test]
fn no_retry_loop_outside_until() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scenarios");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("scenarios dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        if text.contains("loop {") && text.contains("sleep") {
            bad.push(format!("{}: loop with a sleep", path.display()));
        }
        for line in text.lines() {
            if let Some(i) = line.find("from_millis(") {
                let rest = &line[i + "from_millis(".len()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.parse::<u64>().is_ok_and(|ms| ms > 50) {
                    bad.push(format!("{}: sleep of {digits} ms", path.display()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "retry loops outside until: {bad:?}");
}

#[test]
fn filter_words_are_case_insensitive_substrings() {
    let all = ALL;
    let hits = select(all, &["HOOKSMOKE".to_string()]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "hookSmoke");
    let every = select(all, &[]);
    assert_eq!(every.len(), all.len());
    let none = select(all, &["no-such-scenario-xyz".to_string()]);
    assert!(none.is_empty());
}

#[test]
fn the_table_lists_every_scenario_it_ran() {
    let rows = vec![
        Row {
            name: "hookSmoke".into(),
            checks: 3,
            fails: 1,
            skips: 0,
            secs: 1.25,
        },
        Row {
            name: "cwdLabel".into(),
            checks: 0,
            fails: 0,
            skips: 2,
            secs: 0.5,
        },
    ];
    let text = table(&rows);
    assert!(text.lines().next().unwrap().contains("checks"));
    assert!(text.contains("hookSmoke"));
    assert!(text.contains("cwdLabel"));
    assert_eq!(text.lines().count(), 3, "header plus one line per row");
}

#[test]
fn a_timed_out_child_is_killed_and_reported() {
    let sup = supervise(blocking("timeout-test"), Duration::from_millis(500), false);
    assert!(matches!(sup.outcome, Outcome::TimedOut));
    assert!(!pid_alive(sup.pid), "the child is dead after the timeout");
}

#[test]
fn a_missing_exe_exits_1_with_the_build_hint() {
    let out = Command::new(env!("CARGO_BIN_EXE_flare-e2e"))
        .args(["spawn", "--exe", r"C:\flare-e2e-no-such\Flare.exe"])
        .output()
        .expect("run the driver");
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("is missing: run \"cargo build --release -p flare\" first."),
        "{text}"
    );
}

#[test]
fn the_run_one_child_prints_json_lines() {
    let exe = env!("CARGO_BIN_EXE_flare-e2e");
    let out = Command::new(exe)
        .args(["--run-one", "spawn", "--exe", exe])
        .output()
        .expect("run the child");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with('{')).collect();
    assert!(lines.len() >= 2, "a check line and a done line: {text}");
    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(first["pass"], serde_json::Value::Bool(false));
    assert!(first["msg"].as_str().unwrap().contains("not ported"));
    let last: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).expect("json");
    assert_eq!(last["done"], serde_json::Value::Bool(true));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn close_races_a_15_s_timer_then_kills() {
    let profile = profile("close");
    let child = Child::spawn(blocking(&profile.to_string_lossy()), &profile);
    let pid = child.pid();
    // The quit does nothing, so the grace runs out and the tree is killed.
    let exited_clean = close_child(&child, &profile, || {}, Duration::from_millis(300));
    assert!(!exited_clean);
    assert!(child.exited().is_some(), "killed after the grace");
    assert!(!pid_alive(pid));
}

#[test]
fn shots_failures_are_ignored() {
    let dir = profile("shots");
    let ctx = Ctx::new("shots");
    let before = ctx.fails.len();
    let shot = shot_file(&dir, "shots", "failing", |_| Err("no app".to_string()));
    assert!(shot.is_none(), "a failed snapshot is not a result");
    assert_eq!(ctx.fails.len(), before, "and it adds no FAIL");
    let good = shot_file(&dir, "shots", "fine", |_| Ok((4, 2)));
    assert_eq!(good, Some(dir.join("shots-fine.png")));
}

#[test]
fn skip_prints_skip_and_counts_separately() {
    let mut ctx = Ctx::new("skipping");
    ctx.skip("pwsh absent");
    ctx.ok(true, "a pass");
    assert_eq!(ctx.skips, 1);
    assert_eq!(ctx.checks, 1, "a skip is not a check");
    assert!(ctx.fails.is_empty(), "and it is never a fail");
}
