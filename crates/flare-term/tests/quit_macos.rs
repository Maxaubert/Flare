//! Quit and kill with real shells on macOS: none remain (rules 28, 29, 30), and the foreground job
//! says whether a program runs in a tab. The twin of `quit.rs`, with `/bin/sh` in a real pty
//! instead of cmd in the bundled ConPTY (no startup queries to answer).
#![cfg(not(windows))]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{process_alive, temp_dir_named};
use flare_shells::list::ShellDef;
use flare_term::fake::FakeShells;
use flare_term::pty::PortableSpawner;
use flare_term::session::{SessionManager, SessionOptions, SessionSink};

/// Tests that count this process's child shells must not overlap.
static SERIAL: Mutex<()> = Mutex::new(());

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Default)]
struct Sink {
    data: Mutex<Vec<(String, Vec<u8>)>>,
    exits: Mutex<Vec<(String, Option<u32>)>>,
}

impl Sink {
    fn bytes_of(&self, id: &str) -> Vec<u8> {
        lk(&self.data)
            .iter()
            .filter(|(i, _)| i == id)
            .flat_map(|(_, b)| b.iter().copied())
            .collect()
    }

    fn exit_of(&self, id: &str) -> Option<Option<u32>> {
        lk(&self.exits)
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, c)| *c)
    }
}

impl SessionSink for Sink {
    fn data(&self, id: &str, bytes: Vec<u8>) {
        lk(&self.data).push((id.to_string(), bytes));
    }

    fn exit(&self, id: &str, code: Option<u32>) {
        lk(&self.exits).push((id.to_string(), code));
    }
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
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn sh() -> ShellDef {
    ShellDef {
        id: "sh".into(),
        name: "sh".into(),
        exe: "/bin/sh".into(),
        args: vec!["-i".into()],
    }
}

fn manager(def: ShellDef) -> (Arc<SessionManager>, Arc<Sink>) {
    let mgr = SessionManager::new(
        Arc::new(PortableSpawner),
        FakeShells::with(vec![def]),
        SessionOptions::default(),
    );
    (mgr, Arc::new(Sink::default()))
}

/// The pids in the session each shell leads (the shell, its jobs, their children).
fn session_members(shells: &[u32]) -> Vec<u32> {
    flare_proc::table()
        .into_iter()
        .filter(|r| flare_proc::session_of(r.pid).is_some_and(|s| shells.contains(&s)))
        .map(|r| r.pid)
        .collect()
}

#[test]
fn ten_real_shells_with_jobs_killed_none_remain() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-ten");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = manager(sh());
    for i in 0..10 {
        assert!(
            mgr.spawn(
                &format!("t{i}"),
                &root,
                Some("sh"),
                sink.clone(),
                None,
                None
            ),
            "t{i} did not start"
        );
    }
    for i in 0..10 {
        let id = format!("t{i}");
        assert!(
            wait_until(10_000, || !sink.bytes_of(&id).is_empty()),
            "{id} printed nothing"
        );
        // A background job in every tab, and one that ignores the hangup.
        mgr.write(
            &id,
            b"/bin/sleep 120 & /bin/sh -c 'trap \"\" HUP; /bin/sleep 121' &\n",
        );
    }
    let pids: Vec<u32> = mgr.live_pids().into_iter().map(|(_, p)| p).collect();
    assert_eq!(pids.len(), 10, "live pids: {pids:?}");
    assert!(
        wait_until(10_000, || session_members(&pids).len() >= 10 * 4),
        "the jobs did not start: {} members",
        session_members(&pids).len()
    );
    let all = session_members(&pids);
    mgr.kill_all();
    let gone = mgr.shells_gone(Duration::from_secs(3));
    assert!(
        gone,
        "{} shells were still dying after 3 s",
        mgr.shells_dying()
    );
    assert!(
        wait_until(3_000, || all.iter().all(|p| !process_alive(*p))),
        "still alive: {:?}",
        all.iter()
            .filter(|p| process_alive(**p))
            .collect::<Vec<_>>()
    );
}

#[test]
fn kill_while_output_flows_50_times() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-fifty");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = manager(ShellDef {
        id: "spew".into(),
        name: "spew".into(),
        exe: env!("CARGO_BIN_EXE_flare_spew").into(),
        args: vec!["0".into()],
    });
    let mut seen = Vec::new();
    for i in 0..50 {
        let id = format!("s{i}");
        assert!(
            mgr.spawn(&id, &root, None, sink.clone(), None, None),
            "{id} did not start"
        );
        if let Some((_, pid)) = mgr.live_pids().into_iter().find(|(n, _)| *n == id) {
            seen.push(pid);
        }
        std::thread::sleep(Duration::from_millis(30));
        mgr.kill(&id);
    }
    assert!(
        mgr.shells_gone(Duration::from_secs(10)),
        "{} shells still dying after 10 s",
        mgr.shells_dying()
    );
    assert!(
        wait_until(5_000, || mgr.live_pids().is_empty()),
        "live pids remain: {:?}",
        mgr.live_pids()
    );
    assert!(
        wait_until(5_000, || seen.iter().all(|p| !process_alive(*p))),
        "processes remain"
    );
}

#[test]
fn a_warm_spare_is_killed_at_quit() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-spare");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, _sink) = manager(sh());
    let me = std::process::id();
    // `/bin/sh` re-executes into bash, so the spare is found by its argv, not its name.
    let mine = || -> Vec<u32> {
        flare_proc::children(me)
            .into_iter()
            .filter(|p| {
                flare_proc::argv(*p).is_some_and(|a| a.first().is_some_and(|x| x == "/bin/sh"))
            })
            .collect()
    };
    let before = mine();
    mgr.prewarm(&root, Some("sh"), None);
    let fresh = || -> Vec<u32> { mine().into_iter().filter(|p| !before.contains(p)).collect() };
    assert!(
        wait_until(5_000, || !fresh().is_empty()),
        "no spare sh appeared"
    );
    let spare = fresh();
    mgr.kill_all();
    assert!(
        mgr.shells_gone(Duration::from_secs(3)),
        "{} shells still dying",
        mgr.shells_dying()
    );
    assert!(
        wait_until(2_000, || spare.iter().all(|p| !process_alive(*p))),
        "the spare is still alive: {spare:?}"
    );
}

#[test]
fn a_shell_that_exits_by_itself_is_reported_and_removed() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-selfexit");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = manager(sh());
    assert!(mgr.spawn("a", &root, Some("sh"), sink.clone(), None, None));
    assert!(
        wait_until(10_000, || !sink.bytes_of("a").is_empty()),
        "sh printed nothing"
    );
    mgr.write("a", b"exit\n");
    assert!(
        wait_until(10_000, || sink.exit_of("a") == Some(Some(0))),
        "no exit(Some(0)); got {:?}",
        sink.exit_of("a")
    );
    assert!(
        wait_until(2_000, || mgr.live_pids().iter().all(|(id, _)| id != "a")),
        "a is still listed: {:?}",
        mgr.live_pids()
    );
    mgr.kill_all();
    assert!(mgr.shells_gone(Duration::from_secs(3)));
}

#[test]
fn the_foreground_says_whether_a_program_runs_in_the_tab() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-fg");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = manager(sh());
    assert!(mgr.spawn("f", &root, Some("sh"), sink.clone(), None, None));
    assert!(wait_until(10_000, || !sink.bytes_of("f").is_empty()));
    let shell = mgr.live_pids()[0].1;
    assert_eq!(
        mgr.foreground_pid("f"),
        Some(shell),
        "the shell at its prompt"
    );
    mgr.write("f", b"/bin/sleep 30\n");
    // The shell's fork is in front before it has run `/bin/sleep` (its argv is still the shell's
    // for a moment), so this waits for the program itself, never for the first pid that is not the
    // shell's. A busy CI runner can take seconds between the fork and the exec.
    let sleeping =
        |p: u32| p != shell && flare_proc::argv(p).is_some_and(|a| a == ["/bin/sleep", "30"]);
    assert!(
        wait_until(30_000, || mgr.foreground_pid("f").is_some_and(sleeping)),
        "/bin/sleep never came to the foreground: {:?}",
        mgr.foreground_pid("f").map(|p| (p, flare_proc::argv(p)))
    );
    // Ctrl+C ends it, and the shell is in front again.
    mgr.write("f", b"\x03");
    assert!(
        wait_until(10_000, || mgr.foreground_pid("f") == Some(shell)),
        "the shell did not come back"
    );
    assert_eq!(mgr.foreground_pid("nope"), None);
    mgr.kill_all();
    assert!(mgr.shells_gone(Duration::from_secs(3)));
}

/// Helper half: only runs when the parent test sets the variable. Quits the way Flare does (kill
/// every tab, wait for `shells_gone`, exit at once, no destructors), with a job in the tab that
/// ignores SIGHUP. Writes that job's pid first.
#[test]
fn quit_helper_kills_then_exits_at_once() {
    let Ok(out) = std::env::var("FLARE_MAC_QUIT_HELPER_OUT") else {
        return;
    };
    let dir = std::env::temp_dir();
    let root = dir.to_string_lossy().into_owned();
    let (mgr, sink) = manager(sh());
    assert!(mgr.spawn("q", &root, Some("sh"), sink.clone(), None, None));
    assert!(wait_until(10_000, || !sink.bytes_of("q").is_empty()));
    mgr.write("q", b"/bin/sh -c 'trap \"\" HUP; /bin/sleep 331; :' &\n");
    let stubborn = || {
        flare_proc::table()
            .into_iter()
            .filter(|r| {
                flare_proc::argv(r.pid).is_some_and(|a| a.len() == 2 && a[1] == "331")
                    && flare_proc::stat(r.pid).is_some_and(|s| s.ppid != 1)
            })
            .map(|r| r.pid)
            .collect::<Vec<u32>>()
    };
    assert!(wait_until(10_000, || !stubborn().is_empty()), "no job");
    // The sleep and the `sh -c` that runs it.
    let mut pids = stubborn();
    let parents: Vec<u32> = pids
        .iter()
        .filter_map(|p| flare_proc::stat(*p).map(|s| s.ppid))
        .collect();
    pids.extend(parents);
    let text: Vec<String> = pids.iter().map(u32::to_string).collect();
    std::fs::write(&out, text.join(" ")).unwrap();
    mgr.kill_all();
    let t = Instant::now();
    let gone = mgr.shells_gone(Duration::from_secs(3));
    let took = t.elapsed();
    std::fs::write(
        format!("{out}.took"),
        format!("{gone} {}", took.as_millis()),
    )
    .unwrap();
    std::process::exit(0);
}

#[test]
fn quitting_ends_a_job_that_ignores_the_hangup_before_the_process_exits() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-mac-quit-hup");
    let out = dir.path().join("pids.txt");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "quit_helper_kills_then_exits_at_once",
            "--nocapture",
        ])
        .env("FLARE_MAC_QUIT_HELPER_OUT", &out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the helper exits cleanly");
    let pids: Vec<u32> = std::fs::read_to_string(&out)
        .expect("helper wrote its pids")
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert!(!pids.is_empty());
    let took = std::fs::read_to_string(format!("{}.took", out.display())).unwrap();
    // SIGKILL went out before the exit; give the kernel a moment to take them down.
    let gone = wait_until(500, || pids.iter().all(|p| !process_alive(*p)));
    let left: Vec<u32> = pids.iter().copied().filter(|p| process_alive(*p)).collect();
    for p in &left {
        flare_proc::signal(*p, flare_proc::Signal::Kill);
    }
    assert!(gone, "a HUP-ignoring job outlived the quit: {left:?}");
    let (ok, ms) = took.trim().split_once(' ').unwrap();
    assert_eq!(ok, "true", "shells_gone gave up");
    let ms: u64 = ms.parse().unwrap();
    assert!(ms < 1_500, "the quit waited {ms} ms");
}
