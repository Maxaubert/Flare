//! Quit and kill with real shells: none remain (rules 28, 29, 30).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use common::{process_alive, temp_dir_named};
use flare_shells::list::ShellDef;
use flare_term::fake::FakeShells;
use flare_term::pty::PortableSpawner;
use flare_term::session::{RealShells, SessionManager, SessionOptions, SessionSink};

/// Tests that count this process's child shells must not overlap.
static SERIAL: Mutex<()> = Mutex::new(());

const CPR_QUERY: &[u8] = b"\x1b[6n";

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Records output and exits, and answers ConPTY's startup cursor query (no emulator is here to).
struct CprSink {
    mgr: OnceLock<Weak<SessionManager>>,
    data: Mutex<Vec<(String, Vec<u8>)>>,
    answered: Mutex<HashMap<String, usize>>,
    exits: Mutex<Vec<(String, Option<u32>)>>,
    first_data: Mutex<Option<Instant>>,
}

impl CprSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mgr: OnceLock::new(),
            data: Mutex::new(Vec::new()),
            answered: Mutex::new(HashMap::new()),
            exits: Mutex::new(Vec::new()),
            first_data: Mutex::new(None),
        })
    }

    fn attach(&self, mgr: &Arc<SessionManager>) {
        let _ = self.mgr.set(Arc::downgrade(mgr));
    }

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

    fn is_answered(&self, id: &str) -> bool {
        lk(&self.answered).get(id).is_some_and(|n| *n > 0)
    }

    fn saw_data(&self) -> Option<Instant> {
        *lk(&self.first_data)
    }
}

impl SessionSink for CprSink {
    fn data(&self, id: &str, bytes: Vec<u8>) {
        {
            let mut first = lk(&self.first_data);
            if first.is_none() {
                *first = Some(Instant::now());
            }
        }
        lk(&self.data).push((id.to_string(), bytes));
        let total = count(&self.bytes_of(id), CPR_QUERY);
        let mut answered = lk(&self.answered);
        let done = answered.entry(id.to_string()).or_insert(0);
        while *done < total {
            *done += 1;
            if let Some(m) = self.mgr.get().and_then(Weak::upgrade) {
                m.write(id, b"\x1b[1;1R");
            }
        }
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

fn real_manager() -> (Arc<SessionManager>, Arc<CprSink>) {
    let mgr = SessionManager::new(
        Arc::new(PortableSpawner),
        Arc::new(RealShells),
        SessionOptions::default(),
    );
    let sink = CprSink::new();
    sink.attach(&mgr);
    (mgr, sink)
}

/// Process ids of every process named `name` whose parent is this test process.
fn my_children(name: &str) -> Vec<u32> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let me = std::process::id();
    let mut out = Vec::new();
    // SAFETY: the snapshot call takes no pointers; the handle is checked and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: PROCESSENTRY32W is plain data; all-zero is valid before dwSize is set.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot and `entry` a valid out-pointer with dwSize set.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let exe = std::ffi::OsString::from_wide(&entry.szExeFile[..len])
            .to_string_lossy()
            .to_lowercase();
        if exe == name && entry.th32ParentProcessID == me {
            out.push(entry.th32ProcessID);
        }
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: `snap` was created above and is closed exactly once.
    unsafe { CloseHandle(snap) };
    out
}

#[test]
fn ten_real_shells_killed_none_remain() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-m2-ten");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = real_manager();
    for i in 0..10 {
        let ok = mgr.spawn(
            &format!("t{i}"),
            &root,
            Some("cmd"),
            sink.clone(),
            None,
            None,
        );
        assert!(ok, "t{i} did not start");
    }
    // Every shell has printed its startup and the sink has answered ConPTY's cursor query, as a
    // tab with a running emulator would have. Killing a shell BEFORE that answer makes the
    // pseudo console wait about 3 s for it (seen with the last shell of a tight spawn loop).
    for i in 0..10 {
        let id = format!("t{i}");
        assert!(
            wait_until(10_000, || sink.is_answered(&id)),
            "{id} never got its startup query answered"
        );
    }
    let pids: Vec<u32> = mgr.live_pids().into_iter().map(|(_, p)| p).collect();
    assert_eq!(pids.len(), 10, "live pids: {pids:?}");
    mgr.kill_all();
    assert!(
        mgr.shells_gone(Duration::from_secs(3)),
        "{} shells were still dying after 3 s",
        mgr.shells_dying()
    );
    assert!(
        wait_until(1_000, || pids.iter().all(|p| !process_alive(*p))),
        "still alive: {:?}",
        pids.iter()
            .filter(|p| process_alive(**p))
            .collect::<Vec<_>>()
    );
}

#[test]
fn kill_while_output_flows_50_times() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-m2-fifty");
    let root = dir.path().to_string_lossy().into_owned();
    let shells = FakeShells::with(vec![ShellDef {
        id: "spew".into(),
        name: "spew".into(),
        exe: env!("CARGO_BIN_EXE_flare_spew").into(),
        args: vec!["0".into()],
    }]);
    let mgr = SessionManager::new(Arc::new(PortableSpawner), shells, SessionOptions::default());
    let sink = CprSink::new();
    sink.attach(&mgr);
    let mut seen_pids = Vec::new();
    for i in 0..50 {
        let id = format!("s{i}");
        assert!(
            mgr.spawn(&id, &root, None, sink.clone(), None, None),
            "{id} did not start"
        );
        if let Some((_, pid)) = mgr.live_pids().into_iter().find(|(n, _)| *n == id) {
            seen_pids.push(pid);
        }
        std::thread::sleep(Duration::from_millis(30)); // sync-ok: let output flow before the kill
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
        wait_until(5_000, || seen_pids.iter().all(|p| !process_alive(*p))),
        "processes remain"
    );
    // Reaching here means the test process was not taken down (0xc0000409, rule 29).
}

#[test]
fn a_warm_shell_is_adopted_and_the_banner_is_there() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-m2-warm");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = real_manager();
    mgr.prewarm(&root, Some("cmd"), None);
    std::thread::sleep(Duration::from_millis(1_500)); // sync-ok: let the spare print its banner
    assert!(
        mgr.spawn("w", &root, Some("cmd"), sink.clone(), None, None),
        "spawn did not adopt the warm shell"
    );
    assert!(
        wait_until(5_000, || {
            String::from_utf8_lossy(&sink.bytes_of("w")).contains("Microsoft Windows")
        }),
        "no banner; got {:?}",
        String::from_utf8_lossy(&sink.bytes_of("w"))
    );
    mgr.kill_all();
    assert!(mgr.shells_gone(Duration::from_secs(3)));
}

#[test]
fn a_warm_spare_is_killed_at_quit() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-m2-spare");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, _sink) = real_manager();
    let before = my_children("cmd.exe");
    mgr.prewarm(&root, Some("cmd"), None);
    let spare: Vec<u32> = my_children("cmd.exe")
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    assert!(!spare.is_empty(), "no spare cmd.exe appeared");
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
    let dir = temp_dir_named("flare-m2-selfexit");
    let root = dir.path().to_string_lossy().into_owned();
    let (mgr, sink) = real_manager();
    assert!(mgr.spawn("a", &root, Some("cmd"), sink.clone(), None, None));
    assert!(
        wait_until(10_000, || !sink.bytes_of("a").is_empty()),
        "cmd printed nothing"
    );
    std::thread::sleep(Duration::from_millis(500)); // sync-ok: let cmd reach its prompt
    mgr.write("a", b"exit\r");
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
fn spawn_and_first_output_time_is_reported() {
    let _serial = lk(&SERIAL);
    let dir = temp_dir_named("flare-m2-timing");
    let root = dir.path().to_string_lossy().into_owned();

    let (mgr, sink) = real_manager();
    let t = Instant::now();
    assert!(mgr.spawn("cold", &root, Some("cmd"), sink.clone(), None, None));
    let spawned = Instant::now();
    assert!(wait_until(10_000, || sink.saw_data().is_some()));
    let first = sink.saw_data().unwrap_or(spawned);
    println!(
        "cold: spawn returned after {} ms, first output {} ms after that",
        spawned.duration_since(t).as_millis(),
        first.saturating_duration_since(spawned).as_millis()
    );
    mgr.kill_all();
    assert!(mgr.shells_gone(Duration::from_secs(3)));

    let (mgr, sink) = real_manager();
    mgr.prewarm(&root, Some("cmd"), None);
    std::thread::sleep(Duration::from_millis(1_500)); // sync-ok: let the spare start
    let t = Instant::now();
    assert!(mgr.spawn("warm", &root, Some("cmd"), sink.clone(), None, None));
    let spawned = Instant::now();
    assert!(wait_until(10_000, || sink.saw_data().is_some()));
    let first = sink.saw_data().unwrap_or(spawned);
    println!(
        "warm: spawn returned after {} ms, first output {} ms after that",
        spawned.duration_since(t).as_millis(),
        first.saturating_duration_since(spawned).as_millis()
    );
    mgr.kill_all();
    assert!(mgr.shells_gone(Duration::from_secs(3)));
}
