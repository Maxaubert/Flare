//! Task 14: the world folder, the guards and the stall report, each tested with fakes where the
//! real thing would touch the owner's PC (the clipboard, the foreground window, %TEMP%).

use std::cell::Cell;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use flare_diag::reader::Line;
use flare_e2e::guards::{
    run_set, ClipSeq, ClipboardGuard, Foreground, ForegroundGuard, StallReport, TempGuard,
};
use flare_e2e::proc::{pid_alive, reap_by_profile, Child};
use flare_e2e::world::{
    prompt_showing, verdict_said, wait_for_agent_verdict_with, Shell, World, PSREADLINE_MUTE,
};
use serde_json::{json, Map, Value};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A cmd that blocks reading an open pipe and carries `tag` in its command line.
fn dummy(tag: &str) -> Command {
    let mut c = Command::new("cmd.exe");
    c.raw_arg(format!("/c \"set /p x= & rem {tag}\""))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    c
}

/// A fake foreground source: each call answers the next entry of `seq` (the last one repeats).
struct FakeFg {
    seq: Vec<Option<u32>>,
    at: Cell<usize>,
}

impl FakeFg {
    fn new(seq: Vec<Option<u32>>) -> Self {
        Self {
            seq,
            at: Cell::new(0),
        }
    }
}

impl Foreground for FakeFg {
    fn foreground_pid(&self) -> Option<u32> {
        let i = self.at.get();
        self.at.set(i + 1);
        self.seq[i.min(self.seq.len() - 1)]
    }
}

/// A fake clipboard sequence source, answering like `FakeFg`.
struct FakeClip {
    seq: Vec<Option<u32>>,
    at: Cell<usize>,
}

impl ClipSeq for FakeClip {
    fn sequence(&self) -> Option<u32> {
        let i = self.at.get();
        self.at.set(i + 1);
        self.seq[i.min(self.seq.len() - 1)]
    }
}

fn line(kind: &str, extra: Value) -> Line {
    let mut raw = Map::new();
    raw.insert("k".into(), json!(kind));
    if let Value::Object(more) = extra {
        for (k, v) in more {
            raw.insert(k, v);
        }
    }
    Line { ms: 0, raw }
}

/// Sets TEMP and TMP for this test process only (nextest runs each test in its own process).
fn point_temp_at(dir: &Path) {
    // SAFETY: called first thing in a test that runs alone in its process; no other thread reads
    // the environment while it changes.
    unsafe {
        std::env::set_var("TEMP", dir);
        std::env::set_var("TMP", dir);
    }
}

#[test]
fn the_world_removes_itself_and_leaves_nothing_in_temp() {
    let base = tempfile::tempdir().expect("base");
    point_temp_at(base.path());
    let mut temp = TempGuard::in_dir(base.path().to_path_buf());
    temp.begin();
    let root: PathBuf;
    {
        let world = World::new();
        root = world.root.clone();
        assert!(world.profile.is_dir(), "profile folder");
        assert!(
            world.alpha.is_dir() && world.beta.is_dir(),
            "alpha and beta folders"
        );
        assert!(root.starts_with(base.path()), "the world is made in TEMP");
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("flare-e2e-"));
    }
    assert!(!root.exists(), "the world folder is removed on drop");
    assert_eq!(temp.end(), Vec::<String>::new(), "nothing left in TEMP");
}

#[test]
fn the_reaper_spares_a_process_without_the_world_path() {
    let world = World::new();
    let root = world.root.clone();
    let victim = Child::spawn(dummy(&root.to_string_lossy()), &root);
    let spare_profile = std::env::temp_dir().join("flare-e2e-never-matches-spare");
    let spare = Child::spawn(dummy("a-tag-without-the-world"), &spare_profile);
    assert!(victim.wait_for(|| pid_alive(victim.pid()), Duration::from_secs(5)));
    assert!(spare.wait_for(|| pid_alive(spare.pid()), Duration::from_secs(5)));

    let killed = reap_by_profile(&root);

    assert_eq!(killed, 1, "only the process with the world path");
    assert!(victim.wait_for(|| !pid_alive(victim.pid()), Duration::from_secs(5)));
    assert!(
        pid_alive(spare.pid()),
        "the process without the path is spared"
    );
}

#[test]
fn the_foreground_guard_flags_a_window_of_the_run_and_only_notes_another_app() {
    let run = run_set(&[10], &[(10, 1), (11, 10)]);

    // A window of the run takes the foreground: a problem.
    let mut g = ForegroundGuard::new(FakeFg::new(vec![Some(500), Some(500), Some(11)]));
    g.begin();
    g.check("after launch", &run);
    g.check("at the end", &run);
    let problems = g.end();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("took the foreground"), "{problems:?}");

    // Another app (the owner at the PC) takes it: a note, never a problem.
    let mut other = ForegroundGuard::new(FakeFg::new(vec![Some(500), Some(700)]));
    other.begin();
    other.check("at the end", &run);
    assert_eq!(other.end(), Vec::<String>::new());
    assert_eq!(other.notes().len(), 1, "{:?}", other.notes());
}

#[test]
fn the_clipboard_guard_flags_a_sequence_change() {
    let mut same = ClipboardGuard::new(FakeClip {
        seq: vec![Some(5), Some(5)],
        at: Cell::new(0),
    });
    same.begin();
    assert_eq!(same.end(), Vec::<String>::new());

    let mut changed = ClipboardGuard::new(FakeClip {
        seq: vec![Some(5), Some(7)],
        at: Cell::new(0),
    });
    changed.begin();
    let problems = changed.end();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("real clipboard changed"),
        "{problems:?}"
    );
}

#[test]
fn the_temp_guard_reports_only_new_flare_e2e_entries() {
    let base = tempfile::tempdir().expect("base");
    std::fs::create_dir(base.path().join("flare-e2e-already-here")).unwrap();
    std::fs::create_dir(base.path().join("unrelated")).unwrap();
    let mut g = TempGuard::in_dir(base.path().to_path_buf());
    g.begin();
    std::fs::create_dir(base.path().join("flare-e2e-new-leftover")).unwrap();
    std::fs::create_dir(base.path().join("unrelated-new")).unwrap();
    let problems = g.end();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("flare-e2e-new-leftover"),
        "{problems:?}"
    );
}

#[test]
fn the_run_set_is_the_children_and_their_descendants() {
    let table = [(10, 1), (11, 10), (12, 11), (20, 1), (21, 20)];
    let set = run_set(&[10], &table);
    assert!(set.contains(&10) && set.contains(&11) && set.contains(&12));
    assert!(!set.contains(&20) && !set.contains(&21));
}

#[test]
fn the_stall_report_lists_only_ui_stalls_of_1000_ms_or_more() {
    let lines = vec![
        line("main-lag", json!({"ms": 1200, "inflight": []})),
        line("main-lag", json!({"ms": 999})),
        line("ui-stack", json!({"ms": 1000})),
        line("frame-slow", json!({})),
        line("fs-slow", json!({"ms": 2000})),
        line("work-slow", json!({"ms": 3000, "name": "thumbs"})),
        line("panic", json!({"msg": "boom"})),
        line("gone", json!({})),
        line("ipc-slow", json!({"ms": 5000})),
        line("info", json!({"ms": 9000})),
    ];
    let mut report = StallReport::new("lifecycle");
    report.observe(&lines);
    let kinds: Vec<&str> = report.stalls().iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "main-lag",
            "ui-stack",
            "fs-slow",
            "work-slow",
            "panic",
            "gone"
        ]
    );
    assert!(report.stalls().iter().all(|s| !s.expected));
    assert!(!report.end().is_empty());
}

#[test]
fn the_expected_stalls_are_only_the_diag_log_scenarios() {
    let lines = vec![
        line("main-lag", json!({"ms": 1500, "inflight": []})),
        line("panic", json!({"msg": "worker"})),
        line("work-slow", json!({"ms": 1500, "name": "e2e-slow"})),
        line("work-slow", json!({"ms": 1500, "name": "thumbs"})),
    ];
    let mut other = StallReport::new("lifecycle");
    other.observe(&lines);
    assert_eq!(other.stalls().len(), 4);
    assert!(other.stalls().iter().all(|s| !s.expected));

    let mut diag = StallReport::new("diagLog");
    diag.observe(&lines);
    let expected: Vec<bool> = diag.stalls().iter().map(|s| s.expected).collect();
    assert_eq!(expected, [true, true, true, false]);
}

#[test]
fn the_psreadline_mute_line_is_pt_exact() {
    assert_eq!(
        PSREADLINE_MUTE,
        "Set-PSReadLineKeyHandler -Chord 'Ctrl+v','Ctrl+V','Shift+Insert' -ScriptBlock {}; Clear-Host"
    );
}

#[test]
fn the_prompt_matcher_finds_a_prompt_at_the_end_only() {
    assert!(prompt_showing("PS C:\\Users\\x> ", Shell::PowerShell));
    assert!(!prompt_showing("PS C:\\Users\\x> dir", Shell::PowerShell));
    assert!(prompt_showing("C:\\Users\\x>", Shell::Cmd));
    assert!(!prompt_showing("running", Shell::Cmd));
}

#[test]
fn an_agent_stand_in_waits_for_the_first_verdict() {
    // The probe is not there yet (an error), then answers without a verdict, then with one.
    let mut calls = 0;
    let mut probe = || {
        calls += 1;
        match calls {
            1 | 2 => Err("unknown probe agent.state".to_string()),
            3 => Ok(json!({"polled": false})),
            _ => Ok(json!({"polled": true})),
        }
    };
    assert!(wait_for_agent_verdict_with(
        &mut probe,
        Duration::from_secs(5)
    ));
    assert!(verdict_said(&json!({"polled": true})));
    assert!(!verdict_said(&json!({"polled": false})));

    // A probe that never answers is a timeout, never a pass.
    let mut never = || Err::<Value, String>("unknown probe agent.state".to_string());
    assert!(!wait_for_agent_verdict_with(
        &mut never,
        Duration::from_millis(200)
    ));
}
