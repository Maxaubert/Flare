//! The guards: clipboard sequence, foreground, temp leftovers, reaping and stalls (Task 14).
//!
//! The rule for each: a guard that can fail a run returns its problems from `end`; the stall
//! report never fails a run (PT: "a report, not a gate"). The real clipboard, the real foreground
//! window and %TEMP% are reached only through traits, so the tests pass fakes.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use flare_diag::reader::{read_lines, Line};
use serde_json::Value;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::proc::reap_by_profile;

/// The prefix every e2e temp entry carries (the sandbox and the world both use it).
const E2E_PREFIX: &str = "flare-e2e-";
/// The scenario whose own stalls are expected (its BlockUi, Slow and PanicWorker cause them).
pub const DIAG_LOG_SCENARIO: &str = "diagLog";
/// The stall kinds M9 writes (M9 D12), and the time from which one is listed.
pub const STALL_KINDS: [&str; 5] = ["main-lag", "ui-stack", "frame-slow", "fs-slow", "work-slow"];
pub const ERROR_KINDS: [&str; 5] = [
    "panic",
    "work-error",
    "render-error",
    "gone",
    "unresponsive",
];
pub const STALL_MIN_MS: u64 = 1000;

// ---------------------------------------------------------------------------------------------
// The clipboard guard

/// Where the clipboard sequence number comes from.
pub trait ClipSeq {
    /// The number Windows counts every clipboard change with; None when it cannot be read.
    fn sequence(&self) -> Option<u32>;
}

#[cfg(windows)]
/// The real clipboard (`GetClipboardSequenceNumber`; it reads a number and changes nothing).
pub struct WinClipboard;

#[cfg(windows)]
impl ClipSeq for WinClipboard {
    fn sequence(&self) -> Option<u32> {
        // SAFETY: no arguments; the call only reads the sequence counter.
        let n = unsafe { GetClipboardSequenceNumber() };
        (n != 0).then_some(n)
    }
}

/// The owner's real clipboard is not touched: the count is read before the first scenario and
/// after the last. An unreadable count is not a problem (PT prints a note for it).
pub struct ClipboardGuard<C: ClipSeq> {
    src: C,
    before: Option<u32>,
}

impl<C: ClipSeq> ClipboardGuard<C> {
    pub fn new(src: C) -> Self {
        Self { src, before: None }
    }

    pub fn begin(&mut self) {
        self.before = self.src.sequence();
    }

    /// The problems: one, when the count moved during the run.
    pub fn end(&self) -> Vec<String> {
        match (self.before, self.src.sequence()) {
            (Some(b), Some(a)) if a != b => vec![format!(
                "the real clipboard changed during the run ({} change(s)); if you copied something while it ran, run it again",
                a.wrapping_sub(b)
            )],
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The foreground guard

/// Where the foreground window's process comes from.
pub trait Foreground {
    /// The process id that owns the foreground window; None when there is none.
    fn foreground_pid(&self) -> Option<u32>;
}

#[cfg(windows)]
/// The real foreground window (`GetForegroundWindow` and its owner's process id).
pub struct WinForeground;

#[cfg(windows)]
impl Foreground for WinForeground {
    fn foreground_pid(&self) -> Option<u32> {
        // SAFETY: no arguments; the handle is only compared and passed on.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        // SAFETY: hwnd came from GetForegroundWindow a line above; pid is a valid out pointer.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        (pid != 0).then_some(pid)
    }
}

/// The processes of this run: the launched children and everything they started.
/// macOS (rule 1104): the general pasteboard's change count, which moves on any app's write.
#[cfg(target_os = "macos")]
pub struct MacPasteboard;

#[cfg(target_os = "macos")]
impl ClipSeq for MacPasteboard {
    fn sequence(&self) -> Option<u32> {
        crate::mac::pasteboard_change_count()
    }
}

/// macOS (rule 1104): the frontmost app's pid (the app that has the keyboard).
#[cfg(target_os = "macos")]
pub struct MacForeground;

#[cfg(target_os = "macos")]
impl Foreground for MacForeground {
    fn foreground_pid(&self) -> Option<u32> {
        crate::mac::frontmost_pid()
    }
}

/// macOS: every process as (pid, parent pid), from `ps`.
#[cfg(target_os = "macos")]
pub fn process_table() -> Vec<(u32, u32)> {
    crate::proc::process_rows()
        .into_iter()
        .map(|(pid, ppid, _)| (pid, ppid))
        .collect()
}

pub fn run_set(roots: &[u32], table: &[(u32, u32)]) -> HashSet<u32> {
    let mut set: HashSet<u32> = roots.iter().copied().collect();
    loop {
        let before = set.len();
        for &(pid, ppid) in table {
            if set.contains(&ppid) {
                set.insert(pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

#[cfg(windows)]
/// Every running process as `(pid, parent pid)` (a Toolhelp snapshot). Empty when it cannot be taken.
pub fn process_table() -> Vec<(u32, u32)> {
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
        out.push((e.th32ProcessID, e.th32ParentProcessID));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) };
    }
    // SAFETY: closes the snapshot once.
    unsafe {
        CloseHandle(snap);
    }
    out
}

/// The run never takes the foreground. A window of this run in front is a problem; another app
/// (the owner working at the PC) coming forward is a note only.
pub struct ForegroundGuard<F: Foreground> {
    src: F,
    last: Option<u32>,
    problems: Vec<String>,
    notes: Vec<String>,
}

impl<F: Foreground> ForegroundGuard<F> {
    pub fn new(src: F) -> Self {
        Self {
            src,
            last: None,
            problems: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn begin(&mut self) {
        self.last = self.src.foreground_pid();
    }

    /// Looks at the foreground window now. `when` names the moment (after a launch, at the end).
    pub fn check(&mut self, when: &str, run: &HashSet<u32>) {
        let Some(pid) = self.src.foreground_pid() else {
            return;
        };
        if run.contains(&pid) {
            self.problems.push(format!(
                "the scenario took the foreground ({when}, pid {pid})"
            ));
        } else if self.last != Some(pid) {
            self.notes.push(format!(
                "another app took the foreground ({when}, pid {pid}); not a failure"
            ));
        }
        self.last = Some(pid);
    }

    /// The problems found so far.
    pub fn end(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// The notes (another app in front). Never a failure.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

// ---------------------------------------------------------------------------------------------
// The temp guard

/// The `flare-e2e-*` entries of a folder, by name.
fn e2e_entries(dir: &Path) -> BTreeSet<String> {
    let Ok(rd) = fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    rd.flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(E2E_PREFIX))
        .collect()
}

/// No `flare-e2e-*` entry is left in %TEMP% by the run.
pub struct TempGuard {
    dir: PathBuf,
    before: BTreeSet<String>,
}

impl TempGuard {
    /// The guard over the real %TEMP%.
    pub fn in_temp() -> Self {
        Self::in_dir(std::env::temp_dir())
    }

    /// The guard over any folder (the tests use their own).
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            dir,
            before: BTreeSet::new(),
        }
    }

    pub fn begin(&mut self) {
        self.before = e2e_entries(&self.dir);
    }

    /// The problems: one per `flare-e2e-*` entry that is new since `begin`.
    pub fn end(&self) -> Vec<String> {
        e2e_entries(&self.dir)
            .difference(&self.before)
            .map(|n| format!("left in TEMP: {n}"))
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// The reaper

/// After each scenario, every process whose command line holds the world's folder is killed.
pub struct ReapGuard {
    root: PathBuf,
    strays: usize,
}

impl ReapGuard {
    pub fn new(root: PathBuf) -> Self {
        Self { root, strays: 0 }
    }

    /// Kills the strays now. The count is reported, never a failure.
    pub fn reap(&mut self) -> usize {
        self.strays = reap_by_profile(&self.root);
        self.strays
    }

    pub fn strays(&self) -> usize {
        self.strays
    }

    /// The notes for the report: a count when there were strays.
    pub fn end(&self) -> Vec<String> {
        if self.strays == 0 {
            Vec::new()
        } else {
            vec![format!(
                "reaped {} stray process(es) of the world",
                self.strays
            )]
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The stall report

/// One line of the report: a stall or an error from a scenario's diagnostics log.
#[derive(Debug, Clone, PartialEq)]
pub struct Stall {
    pub kind: String,
    pub ms: Option<u64>,
    pub detail: String,
    /// True only for the diag log scenario's own stalls (the ones its BlockUi, Slow and PanicWorker cause).
    pub expected: bool,
}

/// The line `classify` reads for one diag line: a listed stall of `STALL_MIN_MS` or more, or an
/// error kind. None for every other line.
pub fn classify(scenario: &str, line: &Line) -> Option<Stall> {
    let kind = line.raw.get("k").and_then(Value::as_str)?;
    let ms = line.raw.get("ms").and_then(Value::as_u64);
    let is_error = ERROR_KINDS.contains(&kind);
    let is_stall = STALL_KINDS.contains(&kind) && ms.is_some_and(|m| m >= STALL_MIN_MS);
    if !is_error && !is_stall {
        return None;
    }
    let name = line.raw.get("name").and_then(Value::as_str).unwrap_or("");
    let expected = scenario == DIAG_LOG_SCENARIO
        && match kind {
            "main-lag" | "ui-stack" | "frame-slow" | "panic" => true,
            "work-slow" => name == "e2e-slow",
            _ => false,
        };
    Some(Stall {
        kind: kind.to_string(),
        ms,
        detail: detail(&line.raw),
        expected,
    })
}

/// A short, one-line description of a stall (white space collapsed, 70 characters at most).
fn detail(raw: &serde_json::Map<String, Value>) -> String {
    let text = if raw.get("k").and_then(Value::as_str) == Some("main-lag") {
        let flight: Vec<String> = raw
            .get("inflight")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|c| {
                        let ch = c.get("ch").and_then(Value::as_str).unwrap_or("?");
                        let ms = c.get("ms").and_then(Value::as_u64).unwrap_or(0);
                        let done = c.get("done").and_then(Value::as_bool).unwrap_or(false);
                        format!("{ch} {ms}{}", if done { " done" } else { "" })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if flight.is_empty() {
            "nothing in flight".to_string()
        } else {
            flight.join(", ")
        }
    } else {
        let name = raw.get("name").and_then(Value::as_str).unwrap_or("");
        let msg = raw.get("msg").and_then(Value::as_str).unwrap_or("");
        [name, msg]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(70)
        .collect()
}

/// The stalls and errors of one scenario's profile logs. A report, never a gate.
pub struct StallReport {
    scenario: String,
    stalls: Vec<Stall>,
}

impl StallReport {
    pub fn new(scenario: &str) -> Self {
        Self {
            scenario: scenario.to_string(),
            stalls: Vec::new(),
        }
    }

    /// Adds the listed lines of one batch of diag lines.
    pub fn observe(&mut self, lines: &[Line]) {
        self.stalls
            .extend(lines.iter().filter_map(|l| classify(&self.scenario, l)));
    }

    /// Reads a profile's `logs` folder (M9's reader) and adds what it holds.
    pub fn observe_profile(&mut self, profile: &Path) {
        self.observe(&read_lines(&crate::world::logs_dir(profile)));
    }

    pub fn stalls(&self) -> &[Stall] {
        &self.stalls
    }

    /// The report's lines, in PT's layout. `none` when there is nothing to list.
    pub fn end(&self) -> Vec<String> {
        if self.stalls.is_empty() {
            return vec!["  none".to_string()];
        }
        self.stalls
            .iter()
            .map(|s| {
                let ms = s.ms.map_or("-".to_string(), |m| m.to_string());
                let tag = if s.expected { "(expected) " } else { "" };
                format!("  {:<16}{:>7}  {tag}{}", s.kind, ms, s.detail)
            })
            .collect()
    }
}
