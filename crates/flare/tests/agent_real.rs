//! Real-process tests for the agent work (M6 Task 32): the poll over a real process tree, a real
//! ConPTY shell handed the plugin, the real hook script, the resume reaching a stand-in agent, the
//! quit and an output flood.
//!
//! No window is ever opened. Every test has its own temp folders (a temp HOME and a temp PATH folder
//! that holds the stand-in agents), reads nothing of the owner's `~/.claude` or `%APPDATA%\Flare`, and
//! reaps every process it starts. The `AgentHub` is a type of the binary and cannot be linked here,
//! so the tests feed the `flare_agent` `Indicator` (what the hub owns) with the drained events.
//! Every spawn and sleep below is test code (`// sync-ok: test`).
#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use flare_agent::detect::{ProcRow, prefilter};
use flare_agent::indicator::{Event, Indicator, Screen};
use flare_agent::kinds::DetectedAgent;
use flare_agent::plugin::{install_plugin, plugin_dir_in, system_directory};
use flare_agent::poll::{AGENT_FORCE_GAP, AgentPoll, PollDeps};
use flare_agent::signal::{HookState, parse_agent_signal};
use flare_shells::env::PluginEnv;
use flare_term::emulator::{Emulator, TermEvent};
use flare_term::pty::PortableSpawner;
use flare_term::session::{RealShells, SessionManager, SessionOptions, SessionSink};
use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const COLS: usize = 240;
const ROWS: usize = 40;
type Sent = (String, bool, Option<DetectedAgent>);
const SAMPLE_ID: &str = "5a1d0c2e-1111-4222-8333-94445555a666";

/// Tests that start shells or count processes must not overlap (the real machine has one process table).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn wait_until(ms: u64, f: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25)); // sync-ok: test
    }
}

fn process_alive(pid: u32) -> bool {
    // SAFETY: OpenProcess takes no pointers; a null handle means the process is gone.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        // SAFETY: `handle` is live and `code` a valid out-pointer.
        let ok = GetExitCodeProcess(handle, &mut code);
        // SAFETY: `handle` was opened above and is closed once.
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// Ends a process and everything under it.
fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill.exe") // sync-ok: test
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Processes this test started: every tree is ended on drop, panic or not.
#[derive(Default)]
struct Reaper(Vec<u32>);

impl Reaper {
    /// `cmd.exe /d /c <line>`, hidden. `line` is passed raw so its quotes reach cmd as written.
    #[allow(clippy::zombie_processes)] // the tree is ended by the Reaper's drop
    fn spawn_cmd(&mut self, line: &str) -> u32 {
        let child = Command::new("cmd.exe") // sync-ok: test
            .raw_arg(format!("/d /c {line}"))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cmd");
        let pid = child.id();
        self.0.push(pid);
        pid
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for pid in &self.0 {
            kill_tree(*pid);
        }
        let all = self.0.clone();
        wait_until(5_000, || all.iter().all(|p| !process_alive(*p)));
    }
}

/// A temp folder named `name` inside a fresh temp folder (so a path can carry spaces and brackets).
struct NamedDir {
    _base: tempfile::TempDir,
    path: PathBuf,
}

fn named_dir(name: &str) -> NamedDir {
    let base = tempfile::tempdir().expect("temp folder");
    let path = base.path().join(name);
    std::fs::create_dir_all(&path).expect("named folder"); // sync-ok: test
    NamedDir { _base: base, path }
}

/// A stand-in agent: `<dir>\<name>.cmd` with `body` (CRLF).
fn stand_in(dir: &Path, name: &str, body: &str) {
    let text = body.replace('\n', "\r\n");
    std::fs::write(dir.join(format!("{name}.cmd")), text).expect("stand-in"); // sync-ok: test
}

/// The process environment with `path_dir` first on PATH and HOME pointed at a temp folder, so
/// nothing of the owner's `~/.claude` can be reached.
fn test_env(path_dir: &Path, home: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut path_done = false;
    for (k, v) in std::env::vars() {
        let up = k.to_ascii_uppercase();
        if up == "PATH" {
            out.push((k, format!("{};{v}", path_dir.display())));
            path_done = true;
        } else if matches!(up.as_str(), "HOME" | "USERPROFILE" | "FLARE_RESUME_ID") {
            continue;
        } else {
            out.push((k, v));
        }
    }
    if !path_done {
        out.push(("PATH".into(), path_dir.display().to_string()));
    }
    out.push(("HOME".into(), home.display().to_string()));
    out.push(("USERPROFILE".into(), home.display().to_string()));
    out
}

// ---- the live rig: a real SessionManager with an emulator behind every session ----

type Observer = Arc<dyn Fn(&str, &[TermEvent]) + Send + Sync>;

struct Sess {
    emu: Emulator,
    events: Vec<TermEvent>,
    first_data: Instant,
}

/// The sink: feeds the bytes to an emulator, answers its replies (ConPTY waits for them), keeps the
/// events, and tells the observer which events each batch produced.
struct Probe {
    mgr: OnceLock<Weak<SessionManager>>,
    sessions: Mutex<HashMap<String, Sess>>,
    observer: Mutex<Option<Observer>>,
    exits: Mutex<Vec<String>>,
    batches: AtomicUsize,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mgr: OnceLock::new(),
            sessions: Mutex::new(HashMap::new()),
            observer: Mutex::new(None),
            exits: Mutex::new(Vec::new()),
            batches: AtomicUsize::new(0),
        })
    }

    fn screen(&self, id: &str) -> Vec<String> {
        let sessions = lk(&self.sessions);
        sessions.get(id).map_or_else(Vec::new, |s| {
            (0..s.emu.rows())
                .map(|r| s.emu.row_text(i32::try_from(r).unwrap_or(0)))
                .collect()
        })
    }

    fn text(&self, id: &str) -> String {
        self.screen(id).join("\n")
    }

    fn events(&self, id: &str) -> Vec<TermEvent> {
        lk(&self.sessions)
            .get(id)
            .map_or_else(Vec::new, |s| s.events.clone())
    }

    fn first_data(&self, id: &str) -> Option<Instant> {
        lk(&self.sessions).get(id).map(|s| s.first_data)
    }

    fn has_row(&self, id: &str, pred: impl Fn(&str) -> bool) -> bool {
        self.screen(id).iter().any(|r| pred(r))
    }

    fn prompt_shown(&self, id: &str) -> bool {
        self.has_row(id, |r| r.trim_end().ends_with('>'))
    }
}

impl SessionSink for Probe {
    fn data(&self, id: &str, bytes: Vec<u8>) {
        let evs = {
            let mut sessions = lk(&self.sessions);
            let s = sessions.entry(id.to_string()).or_insert_with(|| Sess {
                emu: Emulator::new(COLS, ROWS),
                events: Vec::new(),
                first_data: Instant::now(),
            });
            let mut evs = s.emu.feed(&bytes);
            evs.extend(s.emu.release_sync_if_due(Instant::now()));
            evs.push(TermEvent::Output);
            s.events.extend(
                evs.iter()
                    .filter(|e| !matches!(e, TermEvent::Reply(_)))
                    .cloned(),
            );
            evs
        };
        self.batches.fetch_add(1, Ordering::SeqCst);
        for ev in &evs {
            if let TermEvent::Reply(bytes) = ev
                && let Some(m) = self.mgr.get().and_then(Weak::upgrade)
            {
                m.write(id, bytes);
            }
        }
        let observer = lk(&self.observer).clone();
        if let Some(o) = observer {
            o(id, &evs);
        }
    }

    fn exit(&self, id: &str, _code: Option<u32>) {
        lk(&self.exits).push(id.to_string());
    }
}

struct Live {
    mgr: Arc<SessionManager>,
    probe: Arc<Probe>,
}

impl Live {
    fn new(env: Option<Vec<(String, String)>>) -> Live {
        let opts = SessionOptions {
            base_env: env,
            ..SessionOptions::default()
        };
        let mgr = SessionManager::new(Arc::new(PortableSpawner), Arc::new(RealShells), opts);
        let probe = Probe::new();
        let _ = probe.mgr.set(Arc::downgrade(&mgr));
        Live { mgr, probe }
    }

    fn spawn(
        &self,
        id: &str,
        root: &Path,
        shell: &str,
        resume: Option<&str>,
        plugin: Option<&PluginEnv>,
    ) {
        self.mgr.resize(id, COLS, ROWS);
        let sink: Arc<dyn SessionSink> = self.probe.clone();
        let ok = self.mgr.spawn(
            id,
            &root.to_string_lossy(),
            Some(shell),
            sink,
            resume,
            plugin,
        );
        assert!(ok, "{shell} did not start for {id}");
    }

    fn type_line(&self, id: &str, line: &str) {
        self.mgr.write(id, format!("{line}\r").as_bytes());
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.mgr.kill_all();
        let _ = self.mgr.shells_gone(Duration::from_secs(10));
    }
}

fn have_shell(id: &str) -> bool {
    flare_shells::detect::cached_shells()
        .iter()
        .any(|d| d.id == id)
}

/// A Screen with nothing on it.
struct EmptyScreen;

impl Screen for EmptyScreen {
    fn tail(&self, _tab: &str, _rows: usize) -> Vec<String> {
        Vec::new()
    }
}

// ---- tests ----

#[test]
fn the_real_poll_sees_a_real_agent_process_and_hears_it_leave() {
    let _s = serial();
    let started = Instant::now();
    let mut reaper = Reaper::default();
    // Shell stand-ins: a root cmd with a cmd child that waits on a ping. Nothing carries an agent word.
    let root1 = reaper.spawn_cmd(r#""cmd.exe /d /c ping -n 40 127.0.0.1 >nul""#);
    // The second root's CHILD command line carries the agent's package path (the root's own line is never
    // read; a bare word is not an agent, rule 188).
    let root2 =
        reaper.spawn_cmd(r#""cmd.exe /d /c "ping -n 40 127.0.0.1 >nul & rem @anthropic-ai\claude-code\cli.js"""#);

    let pids: Rc<RefCell<Vec<(String, u32)>>> =
        Rc::new(RefCell::new(vec![("t1".to_string(), root1)]));
    let ticks = Rc::new(Cell::new(0u64));
    let clock = Rc::new(Cell::new(0u64));
    let sent: Rc<RefCell<Vec<Sent>>> = Rc::default();

    let deps = PollDeps {
        pids: {
            let pids = Rc::clone(&pids);
            Box::new(move || pids.borrow().clone())
        },
        ticks: {
            let ticks = Rc::clone(&ticks);
            Box::new(move || {
                ticks.set(ticks.get() + 1);
                ticks.get()
            })
        },
        query: Box::new(|roots| {
            flare_win::procs::snapshot_processes(roots, &prefilter).map(|rows| {
                rows.into_iter()
                    .map(|e| ProcRow {
                        pid: e.pid,
                        ppid: e.ppid,
                        cmd: e.cmd,
                    })
                    .collect()
            })
        }),
        now: {
            let clock = Rc::clone(&clock);
            Box::new(move || clock.get())
        },
        failing: None,
    };
    let send = {
        let sent = Rc::clone(&sent);
        Box::new(move |id: &str, has: bool, kind: Option<DetectedAgent>| {
            sent.borrow_mut().push((id.to_string(), has, kind));
        })
    };
    let mut poll = AgentPoll::new(send, deps);

    // Let the process trees come up before the first look.
    assert!(
        wait_until(10_000, || {
            let rows = flare_win::procs::snapshot_processes(&[root1, root2], &|c| {
                c.contains("claude-code")
            })
            .unwrap_or_default();
            rows.iter().any(|e| e.cmd.contains("claude-code"))
        }),
        "the stand-in tree never showed its command line"
    );

    poll.look(false);
    assert_eq!(
        *sent.borrow(),
        vec![("t1".to_string(), false, None)],
        "nothing under the first root has an agent word"
    );

    // Add the second shell: the next look finds the agent in it.
    pids.borrow_mut().push(("t2".to_string(), root2));
    clock.set(clock.get() + AGENT_FORCE_GAP + 1_000);
    sent.borrow_mut().clear();
    poll.look(true);
    assert_eq!(
        *sent.borrow(),
        vec![("t2".to_string(), true, Some(DetectedAgent::Claude))]
    );

    // Kill the agent-bearing child; the poll hears it leave on a forced look past the gap.
    let child = flare_win::procs::snapshot_processes(&[root2], &|c| c.contains("claude-code"))
        .unwrap_or_default()
        .into_iter()
        .find(|e| e.cmd.contains("claude-code"))
        .expect("the agent-bearing child is still there");
    kill_tree(child.pid);
    assert!(
        wait_until(10_000, || !process_alive(child.pid)),
        "the child did not end"
    );
    clock.set(clock.get() + AGENT_FORCE_GAP + 1_000);
    sent.borrow_mut().clear();
    poll.look(true);
    assert_eq!(*sent.borrow(), vec![("t2".to_string(), false, None)]);

    drop(reaper);
    assert!(
        started.elapsed() < Duration::from_secs(40),
        "the whole test took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_real_conpty_shell_is_handed_the_plugin_and_prints_the_agents_word() {
    let _s = serial();
    let bin = named_dir("bin");
    let home = named_dir("home");
    stand_in(
        &bin.path,
        "claude",
        "@echo off\npowershell -NoProfile -Command \"[Console]::Out.Write([char]27+']777;flare-agent;state=working'+[char]7)\"\n",
    );
    let plugin_src = plugin_dir_in(&[Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("flare-agent")
        .join("claude-plugin")])
    .expect("the bundled plugin folder");
    let plugin_dir = std::fs::canonicalize(&plugin_src)
        .expect("canonical plugin folder") // sync-ok: test
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let plugin = PluginEnv {
        dir: plugin_dir.clone(),
        on: true,
    };
    let live = Live::new(Some(test_env(&bin.path, &home.path)));
    live.spawn("t1", &home.path, "cmd", None, Some(&plugin));
    assert!(
        wait_until(20_000, || live.probe.prompt_shown("t1")),
        "no prompt: {:?}",
        live.probe.screen("t1")
    );
    live.type_line("t1", "echo DIRS=%CLAUDE_CODE_PLUGIN_DIRS%");
    assert!(
        wait_until(20_000, || {
            live.probe.has_row("t1", |r| {
                r.trim_start().starts_with("DIRS=") && r.contains(&plugin_dir)
            })
        }),
        "the plugin dir never reached the shell; screen {:?}",
        live.probe.screen("t1")
    );
    live.type_line("t1", "claude");
    assert!(
        wait_until(20_000, || {
            live.probe.events("t1").iter().any(|e| {
                matches!(e, TermEvent::Osc { ident: 777, data }
                    if parse_agent_signal(data).is_some_and(|s| s.state == HookState::Working))
            })
        }),
        "no working signal came through; events {:?}",
        live.probe.events("t1")
    );

    // The indicator, fed with the drained events as the hub does, has the tab as an agent, working.
    let screen = EmptyScreen;
    let mut ind = Indicator::new();
    for ev in live.probe.events("t1") {
        if let TermEvent::Osc { ident: 777, data } = ev
            && let Some(signal) = parse_agent_signal(&data)
        {
            ind.handle(
                1_000,
                Event::Signal {
                    tab: "t1".into(),
                    signal,
                },
                &screen,
            );
        }
    }
    assert!(ind.agent_ids().contains("t1"));
    assert!(ind.working_ids().contains("t1"));
}

#[test]
fn a_real_hook_script_reaches_the_pty_as_the_parser_reads_it() {
    let _s = serial();
    // The plugin is installed (as the app does) into a folder with spaces and brackets.
    let place = named_dir("Flare (x) T");
    let installed = place.path.join("claude-plugin");
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("flare-agent")
        .join("claude-plugin");
    let system = system_directory().expect("the system folder");
    install_plugin(&src, &installed, &system).expect("install the plugin");
    let hooks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(installed.join("hooks").join("hooks.json")) // sync-ok: test
            .expect("hooks.json"),
    )
    .expect("hooks.json parses");
    let entry = &hooks["hooks"]["Stop"][0]["hooks"][0];
    let command = entry["command"].as_str().expect("command").to_string();
    let root = installed.to_string_lossy().to_string();
    let args: Vec<String> = entry["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|a| {
            a.as_str()
                .expect("arg")
                .replace("${CLAUDE_PLUGIN_ROOT}", &root)
        })
        .collect();
    // Stop names the session too (N25): `done +sid`.
    assert_eq!(args.last().map(String::as_str), Some("+sid"));
    assert_eq!(args[args.len() - 2], "done");
    // The args as Claude Code runs them (exec form): a path with spaces is quoted, nothing else.
    let quoted: Vec<String> = args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect();
    let line = format!("\"{command}\" {}", quoted.join(" "));

    let bin = named_dir("bin");
    let home = named_dir("home");
    // The second stand-in writes the sequence the JSON carries, as Claude Code does into its terminal.
    stand_in(
        &bin.path,
        "emit",
        "@echo off\npowershell -NoProfile -Command \"[Console]::Out.Write([char]27+']777;flare-agent;state=done'+[char]7)\"\n",
    );
    let live = Live::new(Some(test_env(&bin.path, &home.path)));
    live.spawn("t1", &home.path, "cmd", None, None);
    assert!(
        wait_until(20_000, || live.probe.prompt_shown("t1")),
        "no prompt: {:?}",
        live.probe.screen("t1")
    );
    // Claude puts its session id in the hook's environment; the shell stands in for that.
    live.type_line(
        "t1",
        "set CLAUDE_CODE_SESSION_ID=b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9",
    );
    live.type_line("t1", &line);
    assert!(
        wait_until(20_000, || {
            live.probe.has_row("t1", |r| {
                r.trim_start().starts_with("{\"terminalSequence\"")
            })
        }),
        "the hook printed no JSON line; screen {:?}",
        live.probe.screen("t1")
    );
    let json_line = live
        .probe
        .screen("t1")
        .into_iter()
        .find(|r| r.trim_start().starts_with("{\"terminalSequence\""))
        .expect("json row");
    let value: serde_json::Value =
        serde_json::from_str(json_line.trim()).expect("the hook line is JSON");
    assert_eq!(
        value["terminalSequence"].as_str(),
        Some("\u{1b}]777;flare-agent;state=done;session=b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9\u{7}")
    );

    // The same sequence written through ConPTY by a stand-in reaches the parser unchanged.
    live.type_line("t1", "emit");
    assert!(
        wait_until(20_000, || {
            live.probe.events("t1").iter().any(|e| {
                matches!(e, TermEvent::Osc { ident: 777, data } if data == "flare-agent;state=done")
            })
        }),
        "the sequence did not arrive as an Osc event; events {:?}",
        live.probe.events("t1")
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: the timing is of the release build"
)]
fn a_cold_restored_claude_tab_starts_as_fast_as_a_repeat() {
    let _s = serial();
    let shell = if have_shell("pwsh") {
        "pwsh"
    } else {
        "powershell"
    };
    let bin = named_dir("bin");
    let home = named_dir("home");
    stand_in(&bin.path, "claude", "@echo off\necho ARGS:%*\n");
    let def = flare_shells::detect::cached_shells()
        .iter()
        .find(|d| d.id == shell)
        .cloned()
        .expect("the shell");
    let ids = [
        "5a1d0c2e-1111-4222-8333-94445555a666",
        "0f0e0d0c-2222-4333-8444-955566667777",
        "1a2b3c4d-3333-4444-8555-a66677778888",
    ];
    // Three different ids, one command line: Defender judges each NEW command line (rule 3).
    let launches: Vec<_> = ids
        .iter()
        .map(|id| flare_shells::resume::with_resume(&def, id))
        .collect();
    assert_eq!(launches[0].args, launches[1].args);
    assert_eq!(launches[1].args, launches[2].args);
    assert_eq!(launches[0].exe, launches[2].exe);

    let live = Live::new(Some(test_env(&bin.path, &home.path)));
    let mut times = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let tab = format!("t{i}");
        let begun = Instant::now();
        live.spawn(&tab, &home.path, shell, Some(id), None);
        assert!(
            wait_until(30_000, || live.probe.first_data(&tab).is_some()),
            "{tab} printed nothing"
        );
        let first = live
            .probe
            .first_data(&tab)
            .expect("first data")
            .duration_since(begun);
        assert!(
            wait_until(30_000, || {
                live.probe
                    .has_row(&tab, |r| r.contains(&format!("ARGS:--resume {id}")))
            }),
            "{tab} never ran the resume; screen {:?}",
            live.probe.screen(&tab)
        );
        times.push(first);
    }
    eprintln!(
        "cold-restore first output (ms): {:?}",
        times.iter().map(Duration::as_millis).collect::<Vec<_>>()
    );
    assert!(
        times.iter().all(|t| *t < Duration::from_secs(5)),
        "slow start: {times:?}"
    );
    assert!(
        times[2] <= times[0] * 3,
        "the third start ({:?}) took more than 3x the first ({:?})",
        times[2],
        times[0]
    );
}

fn resume_reaches_the_agent(shell: &str) {
    if !have_shell(shell) {
        eprintln!("{shell} is not on this machine; skipped");
        return;
    }
    let bin = named_dir("bin");
    let home = named_dir("home");
    let body = "@echo off\necho ARGS:%*\nif defined FLARE_RESUME_ID echo LEAK\n";
    stand_in(&bin.path, "claude", body);
    stand_in(&bin.path, "codex", body);
    let live = Live::new(Some(test_env(&bin.path, &home.path)));

    live.spawn("c1", &home.path, shell, Some(SAMPLE_ID), None);
    let want = format!("ARGS:--resume {SAMPLE_ID}");
    assert!(
        wait_until(40_000, || live.probe.has_row("c1", |r| r.contains(&want))),
        "{shell}: the agent never got the id; screen {:?}",
        live.probe.screen("c1")
    );
    live.probe.has_row("c1", |_| true);
    // Let any further output land, then: the variable did not follow the id into the agent.
    std::thread::sleep(Duration::from_millis(800)); // sync-ok: test
    assert!(
        !live.probe.text("c1").contains("LEAK"),
        "{shell}: FLARE_RESUME_ID leaked into the agent; screen {:?}",
        live.probe.screen("c1")
    );

    live.spawn("x1", &home.path, shell, Some("codex:last"), None);
    assert!(
        wait_until(40_000, || live
            .probe
            .has_row("x1", |r| r.contains("ARGS:resume --last"))),
        "{shell}: codex did not get `resume --last`; screen {:?}",
        live.probe.screen("x1")
    );
    // Nobody typed into either shell: the test wrote nothing, and the id is not on any typed line.
    assert!(
        !live.probe.text("x1").contains("LEAK"),
        "{shell}: codex saw the variable"
    );
}

#[test]
fn the_resume_id_reaches_the_agent_and_the_variable_does_not() {
    let _s = serial();
    resume_reaches_the_agent("cmd");
    resume_reaches_the_agent("pwsh");
    resume_reaches_the_agent("powershell");
}

fn pid_has_children(parents: &[u32]) -> Vec<u32> {
    let rows = flare_win::procs::snapshot_processes(&[], &|_| false).unwrap_or_default();
    rows.into_iter()
        .filter(|e| parents.contains(&e.ppid))
        .map(|e| e.pid)
        .collect()
}

/// The children of `parents` as `pid:command line`, for a failure message.
fn describe_children(parents: &[u32]) -> Vec<String> {
    let rows = flare_win::procs::snapshot_processes(&[], &|_| true).unwrap_or_default();
    rows.into_iter()
        .filter(|e| parents.contains(&e.ppid))
        .map(|e| format!("{}:{}", e.pid, e.cmd))
        .collect()
}

#[test]
fn quit_waits_for_every_agent_shell() {
    let _s = serial();
    let bin = named_dir("bin");
    let home = named_dir("home");
    // A stand-in agent that keeps running until it is killed.
    stand_in(
        &bin.path,
        "claude",
        "@echo off\necho AGENT-UP\nping -n 120 127.0.0.1 >nul\n",
    );
    let live = Live::new(Some(test_env(&bin.path, &home.path)));
    for i in 0..3 {
        live.spawn(&format!("t{i}"), &home.path, "cmd", None, None);
    }
    for i in 0..3 {
        let id = format!("t{i}");
        assert!(
            wait_until(20_000, || live.probe.prompt_shown(&id)),
            "{id}: no prompt"
        );
        live.type_line(&id, "claude");
    }
    for i in 0..3 {
        let id = format!("t{i}");
        assert!(
            wait_until(20_000, || live
                .probe
                .has_row(&id, |r| r.trim() == "AGENT-UP")),
            "{id}: the stand-in agent did not start; screen {:?}",
            live.probe.screen(&id)
        );
    }
    let shells: Vec<u32> = live.mgr.live_pids().into_iter().map(|(_, p)| p).collect();
    assert_eq!(shells.len(), 3);
    // AGENT-UP is echoed before `ping` starts, so wait until EVERY shell has its child.
    let every_shell_has_a_child = || {
        let kids = flare_win::procs::snapshot_processes(&[], &|_| false).unwrap_or_default();
        shells.iter().all(|s| kids.iter().any(|e| e.ppid == *s))
    };
    assert!(
        wait_until(10_000, every_shell_has_a_child),
        "the shells have no children to lose: {:?}",
        pid_has_children(&shells)
    );

    live.mgr.kill_all();
    // A generous cap, not a sleep: it returns the moment the last shell is gone. Measured under
    // a CPU hog, one run in 50 took over 10 s to kill two ConPTY shells.
    assert!(
        live.mgr.shells_gone(Duration::from_secs(30)),
        "{} shells still dying; alive {:?}; children {:?}",
        live.mgr.shells_dying(),
        shells
            .iter()
            .filter(|p| process_alive(**p))
            .collect::<Vec<_>>(),
        describe_children(&shells)
    );
    assert!(
        wait_until(5_000, || shells.iter().all(|p| !process_alive(*p))),
        "a shell is still alive"
    );
    assert!(
        wait_until(5_000, || pid_has_children(&shells).is_empty()),
        "children of the dead shells remain: {:?}",
        describe_children(&shells)
    );
}

#[test]
fn an_output_flood_from_a_stand_in_agent_is_not_scored_when_hooked() {
    let _s = serial();
    let work = named_dir("flood");
    let home = named_dir("home");
    let bin = named_dir("bin");
    // 20 MB of text for `type` to print.
    let line = "The quick brown fox jumps over the lazy dog 0123456789 The quick brown fox jumps over the lazy dog\r\n";
    let big = work.path.join("flood.txt");
    {
        let mut text = String::with_capacity(1 << 20);
        while text.len() < (1 << 20) {
            text.push_str(line);
        }
        let mut data = Vec::with_capacity(21 << 20);
        while data.len() < 20_000_000 {
            data.extend_from_slice(text.as_bytes());
        }
        std::fs::write(&big, data).expect("flood file"); // sync-ok: test
    }

    // The indicator and its timings live behind one lock; the observer is the "UI-thread drain".
    struct Drain {
        ind: Indicator,
        worst: Duration,
        batches: usize,
    }
    // A Screen that counts every `tail` read, which a hooked flood must never cause.
    let tail_reads = Arc::new(AtomicUsize::new(0));
    struct SyncScreen(Arc<AtomicUsize>);
    impl Screen for SyncScreen {
        fn tail(&self, _tab: &str, _rows: usize) -> Vec<String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        }
    }
    let drain = Arc::new(Mutex::new(Drain {
        ind: Indicator::new(),
        worst: Duration::ZERO,
        batches: 0,
    }));
    let counting = SyncScreen(Arc::clone(&tail_reads));

    let live = Live::new(Some(test_env(&bin.path, &home.path)));
    live.spawn("t1", &home.path, "cmd", None, None);
    assert!(
        wait_until(20_000, || live.probe.prompt_shown("t1")),
        "no prompt"
    );

    // A hook says Working; from here on the session is hooked.
    let signal = parse_agent_signal("flare-agent;state=working").expect("signal");
    {
        let mut d = lk(&drain);
        d.ind.handle(
            1_000,
            Event::Signal {
                tab: "t1".into(),
                signal,
            },
            &counting,
        );
        assert!(d.ind.is_hooked("t1"));
        assert!(d.ind.working_ids().contains("t1"));
    }
    let reads_before = tail_reads.load(Ordering::SeqCst);

    let observer: Observer = {
        let drain = Arc::clone(&drain);
        let counting = SyncScreen(Arc::clone(&tail_reads));
        Arc::new(move |id, evs| {
            if !evs.iter().any(|e| matches!(e, TermEvent::Output)) {
                return;
            }
            let mut d = lk(&drain);
            let begun = Instant::now();
            let at = 2_000 + d.batches as u64;
            d.ind.handle(
                at,
                Event::Output {
                    tab: id.to_string(),
                },
                &counting,
            );
            let took = begun.elapsed();
            d.batches += 1;
            if took > d.worst {
                d.worst = took;
            }
        })
    };
    *lk(&live.probe.observer) = Some(observer);

    live.type_line("t1", &format!("type \"{}\" & echo FLOODEND", big.display()));
    assert!(
        wait_until(120_000, || live
            .probe
            .has_row("t1", |r| r.trim() == "FLOODEND")),
        "the flood never finished; screen tail {:?}",
        live.probe
            .screen("t1")
            .iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
    );
    let d = lk(&drain);
    eprintln!(
        "flood: {} batches, worst indicator drain {:?}",
        d.batches, d.worst
    );
    assert!(d.batches > 10, "the flood arrived in {} batches", d.batches);
    assert_eq!(
        tail_reads.load(Ordering::SeqCst),
        reads_before,
        "a hooked session's output caused a screen read (it was scored)"
    );
    assert!(
        d.worst < Duration::from_millis(50),
        "a batch took {:?} to drain",
        d.worst
    );
    assert!(d.ind.working_ids().contains("t1"), "the tab left Working");
}
