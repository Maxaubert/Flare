//! The runner, the context, the one wait primitive and the report (M10 Task 13).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use flare_e2e::ctx::{close_child, shot_file, Ctx};
use flare_e2e::proc::{pid_alive, Child};
use flare_e2e::report::{table, Row};
use flare_e2e::runner::{pick, select, supervise, Outcome};
use flare_e2e::scenarios::ALL;
use flare_e2e::until::{wait_on, Clock};

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

    // Through the context, `until` records nothing; the timeout becomes one counted FAIL with the
    // wait's name when the scenario's next check, skip or finish comes.
    let mut ctx = Ctx::new("timeout");
    let held = ctx.until("never", 60, &mut || false);
    assert!(!held);
    assert!(ctx.fails.is_empty(), "{:?}", ctx.fails);
    ctx.finish();
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
        // Only a SLEEP over 50 ms is a retry loop's wait: a duration used as a number (a debounce, a
        // sampling interval compared against an elapsed time) is no sleep. A `sleep(` whose argument
        // sits on the next line (rustfmt wraps) counts too.
        let mut after_sleep = false;
        for line in text.lines() {
            let sleeping = line.contains("sleep(") || after_sleep;
            after_sleep = line.trim_end().ends_with("sleep(");
            if !sleeping {
                continue;
            }
            if line.contains("from_secs") {
                bad.push(format!("{}: sleep of whole seconds", path.display()));
            }
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
fn a_plain_run_never_picks_the_opt_in_tour_and_a_filter_that_names_it_does() {
    assert!(pick(&[]).iter().all(|s| s.name != "visualTour"));
    let named: Vec<&str> = pick(&["VISUALTOUR".to_string()])
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(named, vec!["visualTour"]);
    assert!(
        pick(&["hookSmoke".to_string()])
            .iter()
            .all(|s| s.name != "visualTour"),
        "another filter does not pull the tour in"
    );
}

#[test]
fn a_plain_run_never_picks_the_quit_stress_and_naming_it_runs_it_alone() {
    assert!(pick(&[]).iter().all(|s| s.name != "quitStress"));
    let named: Vec<&str> = pick(&["quitStress".to_string()])
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(named, vec!["quitStress"]);
    assert!(
        pick(&["quitManyShells".to_string()])
            .iter()
            .all(|s| s.name != "quitStress"),
        "the gate's quit scenario does not pull the stress in"
    );
}

/// `webBotChecks` opens public pages on the network (#146): only a filter that names it runs it.
#[test]
fn a_plain_run_never_opens_the_public_bot_check_pages() {
    assert!(pick(&[]).iter().all(|s| s.name != "webBotChecks"));
    let named: Vec<&str> = pick(&["webBotChecks".to_string()])
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(named, vec!["webBotChecks"]);
    assert!(
        pick(&["webPanel".to_string()])
            .iter()
            .all(|s| s.name != "webBotChecks"),
        "the web panel's own scenario does not pull it in"
    );
}

/// A part of an opt-in name (`web` for the web panel group, `bot`, `checks`, `tour`) never pulls in
/// the opt-in scenario: only its whole name does.
#[test]
fn a_filter_word_that_is_only_part_of_an_opt_in_name_never_picks_it() {
    for word in [
        "web", "bot", "check", "checks", "e", "WEBBOT", "tour", "visual",
    ] {
        let names: Vec<&str> = pick(&[word.to_string()]).iter().map(|s| s.name).collect();
        assert!(
            !names.contains(&"webBotChecks") && !names.contains(&"visualTour"),
            "filter {word:?} picked an opt-in scenario: {names:?}"
        );
    }
    let web: Vec<&str> = pick(&["web".to_string()]).iter().map(|s| s.name).collect();
    assert!(
        web.contains(&"webPanel"),
        "`web` still runs the web panel group"
    );
    let both: Vec<&str> = pick(&["web".to_string(), "WEBBOTCHECKS".to_string()])
        .iter()
        .map(|s| s.name)
        .collect();
    assert!(both.contains(&"webPanel") && both.contains(&"webBotChecks"));
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
    // Every scenario is ported now, so the child is pointed at an exe that is not there: the launch
    // fails at once and the scenario's returned Err must come out as a failing check line.
    let exe = env!("CARGO_BIN_EXE_flare-e2e");
    let out = Command::new(exe)
        .args([
            "--run-one",
            "spawn",
            "--exe",
            r"C:\flare-e2e-no-such\Flare.exe",
        ])
        .output()
        .expect("run the child");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let values: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert!(values.len() >= 2, "a check line and a done line: {text}");
    let check = values
        .iter()
        .find(|v| v["pass"] == serde_json::Value::Bool(false))
        .unwrap_or_else(|| panic!("a failing check line: {text}"));
    assert!(
        check["msg"].as_str().unwrap().contains("is missing"),
        "{text}"
    );
    let last = values.last().expect("a last line");
    assert_eq!(last["done"], serde_json::Value::Bool(true));
    assert!(last["fails"].as_u64().unwrap() >= 1, "{text}");
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

#[test]
fn a_timeout_followed_by_its_check_is_one_failure_with_both_texts() {
    let mut ctx = Ctx::new("twice");
    let hit = ctx.until("a thing that never comes", 50, &mut || false);
    assert!(!hit);
    ctx.ok(hit, "the thing came");
    assert_eq!(ctx.fails.len(), 1, "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 1, "one check, not two");
    assert!(ctx.fails[0].contains("the thing came"), "{:?}", ctx.fails);
    assert!(
        ctx.fails[0].contains("timed out waiting for a thing that never comes"),
        "{:?}",
        ctx.fails
    );
    // An unrelated failure afterwards is its own.
    ctx.ok(false, "another");
    assert_eq!(ctx.fails.len(), 2);
    assert_eq!(ctx.checks, 2);
}

#[test]
fn a_returned_error_is_kept_after_an_earlier_failure_and_never_said_twice() {
    let mut ctx = Ctx::new("err");
    ctx.ok(false, "an early check failed");
    flare_e2e::runner::keep_error(&mut ctx, "the app did not start");
    assert!(ctx.fails.iter().any(|f| f == "the app did not start"));
    flare_e2e::runner::keep_error(&mut ctx, "the app did not start");
    assert_eq!(ctx.fails.len(), 2, "recorded once: {:?}", ctx.fails);
}

#[test]
fn a_bare_timeout_is_still_one_failure() {
    let mut ctx = Ctx::new("bare");
    assert!(!ctx.until("nothing", 50, &mut || false));
    ctx.ok(true, "a pass");
    assert_eq!(ctx.fails.len(), 1);
}

#[test]
fn until_returns_false_without_recording_anything() {
    let mut ctx = Ctx::new("silent");
    assert!(!ctx.until("never", 50, &mut || false));
    assert!(ctx.fails.is_empty(), "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 0);
}

#[test]
fn a_timeout_with_no_check_after_it_is_one_failure_at_finish() {
    let mut ctx = Ctx::new("finish");
    assert!(!ctx.until("the menu", 50, &mut || false));
    ctx.finish();
    assert_eq!(ctx.fails.len(), 1, "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 1);
    assert!(ctx.fails[0].contains("timed out waiting for the menu"));
}

#[test]
fn a_timeout_then_a_skip_is_not_folded_into_a_later_check() {
    let mut ctx = Ctx::new("skipped");
    assert!(!ctx.until("the tab", 50, &mut || false));
    ctx.skip("not on this box");
    ctx.ok(false, "an unrelated check");
    assert_eq!(ctx.fails.len(), 2, "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 2);
    assert_eq!(ctx.skips, 1);
    assert!(ctx.fails[0].contains("timed out waiting for the tab"));
    assert_eq!(ctx.fails[1], "an unrelated check");
}

#[test]
fn a_timeout_then_a_passing_check_stays_a_failure() {
    let mut ctx = Ctx::new("passing");
    assert!(!ctx.until("the badge", 50, &mut || false));
    ctx.ok(true, "a pass");
    assert_eq!(ctx.fails.len(), 1, "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 2, "the timeout and the pass");
}

#[test]
fn a_returned_error_matches_a_recorded_failure_exactly_not_by_substring() {
    let mut ctx = Ctx::new("exact");
    ctx.ok(false, "the app did not start properly");
    flare_e2e::runner::keep_error(&mut ctx, "the app did not start");
    assert_eq!(ctx.fails.len(), 2, "{:?}", ctx.fails);
    assert_eq!(ctx.fails[1], "the app did not start");
}

#[test]
fn a_returned_error_after_a_timeout_keeps_both_texts_in_one_failure() {
    let mut ctx = Ctx::new("both");
    assert!(!ctx.until("the page", 50, &mut || false));
    flare_e2e::runner::keep_error(&mut ctx, "could not reach the page");
    assert_eq!(ctx.fails.len(), 1, "{:?}", ctx.fails);
    assert_eq!(ctx.checks, 1);
    assert!(ctx.fails[0].contains("could not reach the page"));
    assert!(ctx.fails[0].contains("timed out waiting for the page"));
}

#[test]
fn a_scenario_that_ran_no_check_fails_and_skips_do_not_count() {
    let mut ctx = Ctx::new("empty");
    ctx.skip("everything dropped");
    flare_e2e::runner::require_checks(&mut ctx);
    assert_eq!(ctx.fails, vec![flare_e2e::runner::NO_CHECKS.to_string()]);
    assert_eq!(ctx.skips, 1);
}

#[test]
fn a_scenario_with_a_check_is_left_alone_by_the_empty_guard() {
    let mut ctx = Ctx::new("one");
    ctx.ok(true, "a check");
    flare_e2e::runner::require_checks(&mut ctx);
    assert!(ctx.fails.is_empty());
    assert_eq!(ctx.checks, 1);
}
