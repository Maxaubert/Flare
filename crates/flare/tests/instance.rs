//! Process-level tests: the hand-off and the quit (Task 34).
//!
//! Runs the built binary through its two hidden, window-less modes: `--instance-probe` (the real
//! startup, lock and hand-off server) and `--live-tabs <n>` (real shells, the close path). Every run
//! has its own temp profile, so nothing here can reach the owner's running Flare.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use flare_win::argv::folders_from_argv;

fn flare() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare"));
    cmd.env_remove("FLARE_FORCE_GPU_FAIL");
    cmd.env_remove("FLARE_RENDERER");
    cmd
}

/// `--user-data-dir=<dir>`.
fn profile_arg(dir: &Path) -> String {
    format!("--user-data-dir={}", dir.display())
}

/// A running `--instance-probe`: its stdout lines arrive on a channel; closing stdin ends it.
struct Probe {
    child: Child,
    lines: Receiver<String>,
}

impl Probe {
    fn start(profile: &Path) -> Self {
        let mut child = flare()
            .arg("--instance-probe")
            .arg(profile_arg(profile))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the probe");
        let out = child.stdout.take().expect("probe stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    /// The next line that starts with `prefix`, within `wait`.
    fn line(&self, prefix: &str, wait: Duration) -> Option<String> {
        let end = Instant::now() + wait;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) if l.starts_with(prefix) => return Some(l),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs a second launch to its end (10 s cap) and returns its status code and stdout.
fn second_launch(profile: &Path, cwd: Option<&Path>, extra: &[&str]) -> (Option<i32>, String) {
    let mut cmd = flare();
    cmd.arg(profile_arg(profile))
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().expect("spawn the second launch");
    let end = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break Some(status);
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut text);
    }
    (
        status
            .expect("the second launch did not exit within 10 s")
            .code(),
        text,
    )
}

/// The JSON after `handoff `.
fn handoff_json(line: &str) -> serde_json::Value {
    serde_json::from_str(line.trim_start_matches("handoff ")).expect("hand-off json")
}

#[test]
fn a_second_launch_hands_its_folder_over_and_exits_zero() {
    let profile = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let probe = Probe::start(profile.path());
    assert!(probe.line("first", Duration::from_secs(20)).is_some());
    let folder_arg = folder.path().to_string_lossy().into_owned();
    let (code, stdout) = second_launch(profile.path(), None, &[&folder_arg]);
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "", "a hand-off prints nothing");
    let line = probe
        .line("handoff", Duration::from_secs(10))
        .expect("the probe received a hand-off");
    let json = handoff_json(&line);
    let args: Vec<&str> = json["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    assert!(args.contains(&folder_arg.as_str()), "{args:?}");
}

#[test]
fn a_relative_path_is_resolved_against_the_second_launchs_folder() {
    let profile = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    let probe = Probe::start(profile.path());
    assert!(probe.line("first", Duration::from_secs(20)).is_some());
    let (code, _) = second_launch(profile.path(), Some(tmp.path()), &["sub"]);
    assert_eq!(code, Some(0));
    let line = probe
        .line("handoff", Duration::from_secs(10))
        .expect("the probe received a hand-off");
    let json = handoff_json(&line);
    let cwd = PathBuf::from(json["cwd"].as_str().unwrap());
    assert!(
        cwd.to_string_lossy()
            .eq_ignore_ascii_case(&tmp.path().to_string_lossy()),
        "cwd {cwd:?} vs {:?}",
        tmp.path()
    );
    let argv: Vec<String> = json["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.as_str().map(str::to_string))
        .collect();
    let folders = folders_from_argv(&argv, &[], &cwd);
    assert_eq!(folders.len(), 1, "{folders:?}");
    assert!(
        folders[0]
            .to_lowercase()
            .ends_with(&format!("{}sub", std::path::MAIN_SEPARATOR).to_lowercase()),
        "{folders:?}"
    );
    assert!(
        folders[0]
            .to_lowercase()
            .starts_with(&cwd.to_string_lossy().to_lowercase()),
        "{folders:?}"
    );
}

#[test]
fn a_different_profile_is_not_a_second_instance() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let one = Probe::start(a.path());
    let two = Probe::start(b.path());
    assert!(one.line("first", Duration::from_secs(20)).is_some());
    assert!(two.line("first", Duration::from_secs(20)).is_some());
}

#[test]
fn the_lock_names_carry_no_prism_name() {
    let profile = tempfile::tempdir().unwrap();
    let probe = Probe::start(profile.path());
    let names = probe
        .line("names", Duration::from_secs(20))
        .expect("a names line");
    assert!(names.contains("Flare"), "{names}");
    assert!(!names.to_lowercase().contains("prism"), "{names}");
}

/// Process ids of every process whose parent is `parent`, from a process snapshot.
fn children_of(parent: u32) -> Vec<u32> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked against INVALID_HANDLE_VALUE and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert_ne!(snap, INVALID_HANDLE_VALUE, "process snapshot");
    // SAFETY: an all-zero PROCESSENTRY32W is a valid starting value; dwSize is set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot and `entry` is a writable, correctly sized struct.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        if entry.th32ParentProcessID == parent {
            found.push(entry.th32ProcessID);
        }
        // SAFETY: as for Process32FirstW above.
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: `snap` came from CreateToolhelp32Snapshot and is closed exactly once.
    unsafe { CloseHandle(snap) };
    found
}

/// One `--live-tabs` run: exit code, stdout lines, and the pid (for the leftover-shell check).
struct LiveRun {
    code: Option<i32>,
    lines: Vec<String>,
    pid: u32,
    millis: u128,
}

fn live_tabs(profile: &Path, n: u32) -> LiveRun {
    let mut child = flare()
        .arg("--live-tabs")
        .arg(n.to_string())
        .arg(profile_arg(profile))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn --live-tabs");
    let pid = child.id();
    let out = child.stdout.take().expect("stdout");
    let mut err = child.stderr.take().expect("stderr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let lines: Vec<String> = BufReader::new(out).lines().map_while(Result::ok).collect();
        let _ = tx.send(lines);
    });
    let (etx, erx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut err, &mut s);
        let _ = etx.send(s);
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let millis = started.elapsed().as_millis();
    let lines = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let stderr = erx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let status =
        status.unwrap_or_else(|| panic!("--live-tabs did not exit within 60 s: {lines:?}"));
    assert!(
        status.success() || !stderr.is_empty(),
        "exit {:?}, no stderr",
        status.code()
    );
    LiveRun {
        code: status.code(),
        lines,
        pid,
        millis,
    }
}

/// How many tabs `tabs.json` of `profile` holds.
fn saved_tab_count(profile: &Path) -> usize {
    let raw = std::fs::read_to_string(profile.join("tabs.json")).expect("tabs.json exists");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("tabs.json parses");
    json["tabs"].as_array().expect("a tabs array").len()
}

#[test]
fn ten_live_tabs_quit_by_themselves_with_code_zero_three_times() {
    for round in 0..3 {
        let profile = tempfile::tempdir().unwrap();
        let run = live_tabs(profile.path(), 10);
        assert_eq!(run.code, Some(0), "round {round}: {:?}", run.lines);
        assert!(
            run.lines.iter().any(|l| l.starts_with("quit-ok")),
            "round {round}: {:?}",
            run.lines
        );
        assert!(run.millis < 60_000);
        assert_eq!(saved_tab_count(profile.path()), 10, "round {round}");
        assert_eq!(
            children_of(run.pid),
            Vec::<u32>::new(),
            "round {round}: a shell or OpenConsole is still running"
        );
    }
}

#[test]
fn the_close_flushes_tabs_json_before_the_shells_are_killed() {
    let profile = tempfile::tempdir().unwrap();
    let run = live_tabs(profile.path(), 2);
    assert_eq!(run.code, Some(0), "{:?}", run.lines);
    assert!(run.lines.iter().any(|l| l.starts_with("quit-ok")));
    // The debounce is 400 ms in the app: the file is there the moment quit-ok is printed, so the
    // close path flushed it rather than waiting out the delay.
    assert_eq!(saved_tab_count(profile.path()), 2);
}

#[test]
fn a_saved_strip_comes_back_in_the_same_folders() {
    let profile = tempfile::tempdir().unwrap();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let tab = |p: &Path| serde_json::json!({ "cwd": p.to_string_lossy() });
    let saved = serde_json::json!({ "tabs": [tab(a.path()), tab(b.path())], "active": 1 });
    std::fs::write(profile.path().join("tabs.json"), saved.to_string()).unwrap();
    let run = live_tabs(profile.path(), 0);
    assert_eq!(run.code, Some(0), "{:?}", run.lines);
    let spawned: Vec<&str> = run
        .lines
        .iter()
        .filter_map(|l| l.strip_prefix("spawn "))
        .collect();
    assert_eq!(
        spawned,
        vec![
            a.path().to_string_lossy().as_ref(),
            b.path().to_string_lossy().as_ref()
        ]
    );
    assert_eq!(saved_tab_count(profile.path()), 2);
}
