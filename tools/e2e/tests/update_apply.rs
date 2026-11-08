//! M9 Task 38: the update applied for real, against packages on disk (CI only).
//!
//! `FLARE_E2E_INSTALL=1` runs the three install scenarios on CI (`GITHUB_ACTIONS` set); a local run
//! also needs `FLARE_E2E_INSTALL_LOCAL=1`, because they write the machine's Explorer verb and
//! Velopack keys. Without it they return at once (and are `#[ignore]` too).
//!
//! The inputs, all made by `.github/workflows/update-e2e.yml`:
//!
//! - `FLARE_PACKED_DIR`: a `Releases` folder with `Flare-win-Setup.exe` and the `0.0.1` and `0.0.2`
//!   packages of a `live-updates` build.
//! - `FLARE_PACKED_BIG_DIR`: a folder whose `0.0.2` full package carries a 120 MB incompressible
//!   filler (only the cancel scenario uses it).
//!
//! Layout facts the scenarios rely on (read from the committed code, not from the plan):
//! the e2e files (`update-state.json`, `update-calls.json`, `update-command`) sit directly in the
//! profile folder, and the restart after an install carries only `--user-data-dir=<profile>`, so
//! the restarted app is a normal launch that writes no e2e files; the scenarios therefore prove the
//! restart through the manifest, the old pid and the process list.
//!
//! The tests that need no install (the throttled server and the small helpers) run everywhere.
//!
//! Windows-only: the e2e harness drives the Windows app (job objects, the hidden desktop, named pipes).
//! The Mac E2E is the port plan's "Ongoing" row.
#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_e2e::proc::{pid_alive, reap_by_profile, Child};
use flare_e2e::sandbox::Sandbox;
use serde_json::Value;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CHUNK: usize = 64 * 1024;
const CHUNK_PAUSE: Duration = Duration::from_millis(10);

// ---- pure helpers (tested below without an install) -------------------------------------------

/// `releases.win.json` with every `Delta` asset removed (Velopack downloads deltas with no
/// progress events, which would make the cancel scenario unreliable).
fn without_deltas(json: &str) -> String {
    let mut doc: Value = serde_json::from_str(json).expect("releases.win.json is JSON");
    if let Some(a) = doc.get_mut("Assets").and_then(Value::as_array_mut) {
        a.retain(|x| x.get("Type").and_then(Value::as_str) != Some("Delta"));
    }
    doc.to_string()
}

/// The request path without its query, when the line is a plain `GET`.
fn get_path(request_line: &str) -> Option<String> {
    let mut it = request_line.split_whitespace();
    if it.next()? != "GET" {
        return None;
    }
    let target = it.next()?;
    let path = target.split('?').next()?;
    Some(path.to_string())
}

/// A file name that is one plain component of the served folder, else None.
fn safe_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix('/')?;
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
        return None;
    }
    Some(name)
}

/// The `<version>` of a `sq.version` nuspec.
fn manifest_version(text: &str) -> Option<String> {
    let a = text.find("<version>")? + "<version>".len();
    let b = text[a..].find("</version>")? + a;
    Some(text[a..b].trim().to_string())
}

// ---- the throttled loopback server ------------------------------------------------------------

/// Serves a `Releases` folder over `127.0.0.1` at about 6 MB/s (64 KB every 10 ms) and counts what
/// it served. `releases.win.json` is answered with the deltas removed.
struct Throttle {
    port: u16,
    served: Arc<Mutex<Vec<String>>>,
    foreign: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Throttle {
    fn start(folder: &Path) -> Throttle {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let served = Arc::new(Mutex::new(Vec::new()));
        let foreign = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, f, st, dir) = (
            Arc::clone(&served),
            Arc::clone(&foreign),
            Arc::clone(&stop),
            folder.to_path_buf(),
        );
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, peer)) => {
                        let (s, f, st, dir) =
                            (Arc::clone(&s), Arc::clone(&f), Arc::clone(&st), dir.clone());
                        std::thread::spawn(move || handle(conn, peer, &dir, &s, &f, &st));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Throttle {
            port,
            served,
            foreign,
            stop,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// The paths asked for, in order.
    fn served(&self) -> Vec<String> {
        self.served.lock().unwrap().clone()
    }

    fn foreign_peers(&self) -> usize {
        self.foreign.load(Ordering::SeqCst)
    }
}

impl Drop for Throttle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn respond(conn: &mut TcpStream, status: &str, len: usize) -> bool {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {len}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
    );
    conn.write_all(head.as_bytes()).is_ok()
}

fn handle(
    mut conn: TcpStream,
    peer: SocketAddr,
    dir: &Path,
    served: &Mutex<Vec<String>>,
    foreign: &AtomicUsize,
    stop: &AtomicBool,
) {
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
    if !matches!(peer.ip(), IpAddr::V4(ip) if ip.is_loopback()) {
        foreign.fetch_add(1, Ordering::SeqCst);
        respond(&mut conn, "403 Forbidden", 0);
        return;
    }
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match conn.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let line = text.lines().next().unwrap_or("");
    let Some(path) = get_path(line) else {
        respond(&mut conn, "405 Method Not Allowed", 0);
        return;
    };
    let Some(name) = safe_name(&path) else {
        respond(&mut conn, "404 Not Found", 0);
        return;
    };
    served.lock().unwrap().push(name.to_string());
    let file = dir.join(name);
    if name == "releases.win.json" {
        let Ok(raw) = std::fs::read_to_string(&file) else {
            respond(&mut conn, "404 Not Found", 0);
            return;
        };
        let body = without_deltas(&raw);
        if respond(&mut conn, "200 OK", body.len()) {
            let _ = conn.write_all(body.as_bytes());
        }
        return;
    }
    let Ok(mut f) = std::fs::File::open(&file) else {
        respond(&mut conn, "404 Not Found", 0);
        return;
    };
    let len = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
    if !respond(&mut conn, "200 OK", len) {
        return;
    }
    let mut chunk = vec![0u8; CHUNK];
    while !stop.load(Ordering::SeqCst) {
        let n = match f.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if conn.write_all(&chunk[..n]).is_err() {
            break; // the client cancelled
        }
        std::thread::sleep(CHUNK_PAUSE);
    }
}

// ---- the scenarios' tools -----------------------------------------------------------------------

/// Whether the install scenarios may run: asked for AND on CI (`GITHUB_ACTIONS`) or explicitly
/// opted in with `FLARE_E2E_INSTALL_LOCAL=1`. They write the Explorer verb and the Velopack keys
/// of the machine they run on, so a stray local `FLARE_E2E_INSTALL=1` must not be enough (L53).
fn install_allowed(install: Option<&str>, ci: Option<&str>, local: Option<&str>) -> bool {
    install == Some("1") && (ci.is_some_and(|v| !v.is_empty()) || local == Some("1"))
}

fn install_enabled() -> bool {
    let get = |k: &str| std::env::var(k).ok();
    install_allowed(
        get("FLARE_E2E_INSTALL").as_deref(),
        get("GITHUB_ACTIONS").as_deref(),
        get("FLARE_E2E_INSTALL_LOCAL").as_deref(),
    )
}

fn env_dir(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
}

fn until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs the one-click setup silently into `inst`, then ends whatever it launched (the setup
/// starts the app on the default profile; nothing of ours may stay running).
fn install(releases: &Path, inst: &Path) {
    let mut cmd = Command::new(releases.join("Flare-win-Setup.exe"));
    cmd.arg("--silent")
        .arg("--installto")
        .arg(inst)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let mut setup = cmd.spawn().expect("start the setup");
    let ok = until(Duration::from_secs(180), || {
        setup.try_wait().ok().flatten().is_some()
    });
    if !ok {
        let _ = setup.kill();
        panic!("the setup did not finish in 180 s");
    }
    assert!(
        setup.wait().expect("setup status").success(),
        "the setup failed"
    );
    assert!(
        until(Duration::from_secs(60), || inst
            .join("current")
            .join("Flare.exe")
            .is_file()),
        "no current\\Flare.exe after the setup"
    );
    for _ in 0..16 {
        reap_by_profile(inst);
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn installed_version(inst: &Path) -> Option<String> {
    let text = std::fs::read_to_string(inst.join("current").join("sq.version")).ok()?;
    manifest_version(&text)
}

fn state(profile: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(profile.join("update-state.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn calls(profile: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(profile.join("update-calls.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the e2e command word and waits for the app to take it (it deletes the file).
fn send(profile: &Path, word: &str) {
    let path = profile.join("update-command");
    assert!(
        until(Duration::from_secs(5), || !path.exists()),
        "an earlier command was never taken"
    );
    let tmp = profile.join("update-command.tmp");
    std::fs::write(&tmp, word).expect("write the command");
    std::fs::rename(&tmp, &path).expect("write the command");
    assert!(
        until(Duration::from_secs(10), || !path.exists()),
        "the app never took `{word}`"
    );
}

/// `(pid, command line)` of every `Flare.exe` whose command line holds `profile`.
fn flares_for(profile: &Path) -> Vec<(u32, String)> {
    let script = "Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'Flare.exe' } | ForEach-Object { '{0}|{1}' -f $_.ProcessId, $_.CommandLine }";
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("list processes");
    let needle = profile.to_string_lossy().replace('/', "\\").to_lowercase();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, cmd) = l.split_once('|')?;
            let hit = cmd.replace('/', "\\").to_lowercase().contains(&needle);
            if !hit {
                return None;
            }
            Some((pid.trim().parse().ok()?, cmd.to_string()))
        })
        .collect()
}

/// Ends everything under the install and the profile when a scenario ends, pass or panic.
struct Reaper(Vec<PathBuf>);

impl Drop for Reaper {
    fn drop(&mut self) {
        for p in &self.0 {
            reap_by_profile(p);
        }
    }
}

fn app_env(sb: &Sandbox, source: &str) -> Vec<(String, String)> {
    let mut env = sb.env();
    env.push(("FLARE_E2E_UPDATE_SOURCE".into(), source.to_string()));
    env
}

/// Waits for the restart: the manifest says 0.0.2, the old pid is gone, and exactly one Flare for
/// this profile runs, with the restart arguments. Returns the new pid.
fn assert_restarted(inst: &Path, profile: &Path, old_pid: u32) -> u32 {
    assert!(
        until(Duration::from_secs(120), || installed_version(inst)
            .as_deref()
            == Some("0.0.2")),
        "the install never reached 0.0.2 (manifest: {:?})",
        installed_version(inst)
    );
    assert!(
        until(Duration::from_secs(30), || !pid_alive(old_pid)),
        "the old app is still running"
    );
    let mut now = Vec::new();
    let ok = until(Duration::from_secs(60), || {
        now = flares_for(profile);
        now.len() == 1 && now[0].0 != old_pid
    });
    assert!(
        ok,
        "expected one restarted Flare for the profile, got {now:?}"
    );
    let want = format!("--user-data-dir={}", profile.display()).to_lowercase();
    assert!(
        now[0].1.to_lowercase().contains(&want),
        "the restart lost the profile argument: {}",
        now[0].1
    );
    // It stays exactly one.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(flares_for(profile).len(), 1);
    now[0].0
}

// ---- tests that need no install -------------------------------------------------------------------

#[test]
fn the_install_scenarios_need_ci_or_an_explicit_local_opt_in() {
    // L53: they write the real Explorer verb and Velopack keys of the machine they run on.
    assert!(!install_allowed(None, Some("true"), None));
    assert!(!install_allowed(Some("1"), None, None));
    assert!(!install_allowed(Some("1"), Some(""), None));
    assert!(!install_allowed(Some("1"), None, Some("0")));
    assert!(install_allowed(Some("1"), Some("true"), None));
    assert!(install_allowed(Some("1"), None, Some("1")));
}

#[test]
fn deltas_are_removed_from_the_feed_and_everything_else_is_kept() {
    let json = r#"{"Assets":[
        {"PackageId":"Flare","Version":"0.0.2","Type":"Full","FileName":"Flare-0.0.2-full.nupkg"},
        {"PackageId":"Flare","Version":"0.0.2","Type":"Delta","FileName":"Flare-0.0.2-delta.nupkg"},
        {"PackageId":"Flare","Version":"0.0.1","Type":"Full","FileName":"Flare-0.0.1-full.nupkg"}]}"#;
    let out: Value = serde_json::from_str(&without_deltas(json)).unwrap();
    let names: Vec<&str> = out["Assets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["FileName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Flare-0.0.2-full.nupkg", "Flare-0.0.1-full.nupkg"]);
}

#[test]
fn only_a_plain_get_of_one_file_name_is_served() {
    assert_eq!(
        get_path("GET /releases.win.json?id=Flare&localVersion=0.0.1 HTTP/1.1").as_deref(),
        Some("/releases.win.json")
    );
    assert_eq!(get_path("POST /x HTTP/1.1"), None);
    assert_eq!(
        safe_name("/Flare-0.0.2-full.nupkg"),
        Some("Flare-0.0.2-full.nupkg")
    );
    for bad in ["/", "/../x", "/a/b", "/a\\b", "/C:x", ""] {
        assert_eq!(safe_name(bad), None, "{bad}");
    }
}

#[test]
fn the_manifest_version_is_read_from_the_nuspec() {
    let text = "<package><metadata><id>Flare</id>\n<version> 0.0.2 </version></metadata></package>";
    assert_eq!(manifest_version(text).as_deref(), Some("0.0.2"));
    assert_eq!(manifest_version("<package/>"), None);
}

#[test]
fn the_server_throttles_counts_and_hides_deltas() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("releases.win.json"),
        r#"{"Assets":[{"Type":"Full","FileName":"a.bin"},{"Type":"Delta","FileName":"d.bin"}]}"#,
    )
    .unwrap();
    // 8 chunks of 64 KB: at 10 ms per chunk the body cannot arrive in under 70 ms.
    std::fs::write(dir.path().join("a.bin"), vec![7u8; CHUNK * 8]).unwrap();
    let server = Throttle::start(dir.path());

    let get = |path: &str| -> (Vec<u8>, Duration) {
        let t = Instant::now();
        let mut c = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        c.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .unwrap();
        let mut all = Vec::new();
        c.read_to_end(&mut all).unwrap();
        (all, t.elapsed())
    };
    let (feed, _) = get("/releases.win.json?x=1");
    let feed = String::from_utf8_lossy(&feed).into_owned();
    assert!(feed.contains("a.bin") && !feed.contains("d.bin"), "{feed}");
    let (file, took) = get("/a.bin");
    assert!(file.len() > CHUNK * 8, "headers plus the body");
    assert!(took >= Duration::from_millis(70), "{took:?}");
    let (nope, _) = get("/..%2f");
    assert!(String::from_utf8_lossy(&nope).starts_with("HTTP/1.1 404"));
    assert_eq!(server.served(), ["releases.win.json", "a.bin"]);
    assert_eq!(server.foreign_peers(), 0);
    assert!(server.url().starts_with("http://127.0.0.1:"));
}

// ---- the install scenarios (CI only) ---------------------------------------------------------------

#[test]
#[ignore = "installs Flare: FLARE_E2E_INSTALL=1, FLARE_PACKED_DIR (CI only)"]
fn an_offered_update_is_installed_and_the_app_restarts_on_its_profile() {
    if !install_enabled() {
        return;
    }
    let releases = env_dir("FLARE_PACKED_DIR");
    let sb = Sandbox::new();
    let inst = sb.root.join("inst");
    let profile = sb.root.join("p");
    std::fs::create_dir_all(&profile).unwrap();
    let _reap = Reaper(vec![inst.clone(), profile.clone()]);
    install(&releases, &inst);
    assert_eq!(installed_version(&inst).as_deref(), Some("0.0.1"));

    let exe = inst.join("current").join("Flare.exe");
    let env = app_env(&sb, &releases.to_string_lossy());
    let app = Child::launch(&exe, &[], &env, &profile);
    let old_pid = app.pid();

    assert!(
        until(Duration::from_secs(60), || state(&profile).is_some_and(
            |s| { s["chip"] == true && s["version"] == "0.0.2" && s["mock"] == false }
        )),
        "no real 0.0.2 offer: {:?}",
        state(&profile)
    );
    send(&profile, "open");
    assert!(until(Duration::from_secs(10), || state(&profile)
        .is_some_and(|s| s["open"] == true)));
    send(&profile, "install");

    let new_pid = assert_restarted(&inst, &profile, old_pid);
    assert_ne!(new_pid, old_pid);
    drop(app);
}

#[test]
#[ignore = "installs Flare: FLARE_E2E_INSTALL=1, FLARE_PACKED_DIR (CI only)"]
fn a_downloaded_package_is_never_applied_without_the_install_command() {
    if !install_enabled() {
        return;
    }
    let releases = env_dir("FLARE_PACKED_DIR");
    let sb = Sandbox::new();
    let inst = sb.root.join("inst");
    let profile = sb.root.join("p");
    let empty_feed = sb.root.join("empty-feed");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::create_dir_all(&empty_feed).unwrap();
    let _reap = Reaper(vec![inst.clone(), profile.clone()]);
    install(&releases, &inst);
    assert_eq!(installed_version(&inst).as_deref(), Some("0.0.1"));

    // The 0.0.2 full package is already in the install's packages folder before the launch.
    let packages = inst.join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    let name = "Flare-0.0.2-full.nupkg";
    std::fs::copy(releases.join(name), packages.join(name)).expect("copy the package in");

    let exe = inst.join("current").join("Flare.exe");
    let env = app_env(&sb, &empty_feed.to_string_lossy());
    let app = Child::launch(&exe, &[], &env, &profile);
    assert!(
        until(Duration::from_secs(30), || state(&profile).is_some()),
        "the app never wrote its state"
    );
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(1));
        assert!(app.exited().is_none(), "the app exited");
    }
    assert_eq!(installed_version(&inst).as_deref(), Some("0.0.1"));
    assert_eq!(
        flares_for(&profile).len(),
        1,
        "one app, not a restarted one"
    );
    assert_eq!(
        calls(&profile).map(|c| c["installs"].clone()),
        Some(0.into())
    );
}

#[test]
#[ignore = "installs Flare: FLARE_E2E_INSTALL=1, FLARE_PACKED_DIR, FLARE_PACKED_BIG_DIR (CI only)"]
fn cancel_leaves_nothing_and_a_second_install_still_works() {
    if !install_enabled() {
        return;
    }
    let releases = env_dir("FLARE_PACKED_DIR");
    let big = env_dir("FLARE_PACKED_BIG_DIR");
    let sb = Sandbox::new();
    let inst = sb.root.join("inst");
    let profile = sb.root.join("p");
    std::fs::create_dir_all(&profile).unwrap();
    let _reap = Reaper(vec![inst.clone(), profile.clone()]);
    install(&releases, &inst);
    assert_eq!(installed_version(&inst).as_deref(), Some("0.0.1"));

    let server = Throttle::start(&big);
    let exe = inst.join("current").join("Flare.exe");
    let env = app_env(&sb, &server.url());
    let app = Child::launch(&exe, &[], &env, &profile);
    let old_pid = app.pid();

    assert!(
        until(Duration::from_secs(60), || state(&profile)
            .is_some_and(|s| s["chip"] == true && s["version"] == "0.0.2")),
        "no 0.0.2 offer from the loopback feed: {:?}",
        state(&profile)
    );
    send(&profile, "install");
    assert!(
        until(Duration::from_secs(60), || state(&profile).is_some_and(
            |s| { s["phase"] == "downloading" && s["pct"].as_u64().unwrap_or(0) >= 5 }
        )),
        "the download never reached 5 percent: {:?}",
        state(&profile)
    );
    send(&profile, "cancel");

    let packages = inst.join("packages");
    let leftovers = || -> Vec<String> {
        std::fs::read_dir(&packages)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.contains("0.0.2") || n.ends_with(".partial"))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(
        until(Duration::from_secs(30), || state(&profile).is_some_and(
            |s| { s["phase"] == "idle" && s["chip"] == true }
        ) && leftovers().is_empty()),
        "cancel left state {:?} and files {:?}",
        state(&profile),
        leftovers()
    );
    assert!(app.exited().is_none(), "the app is still running");
    assert_eq!(installed_version(&inst).as_deref(), Some("0.0.1"));

    // The busy flag cleared: a second install runs to the end.
    send(&profile, "install");
    assert_restarted(&inst, &profile, old_pid);

    // The server served the feed and the one package, only to 127.0.0.1.
    let mut seen = server.served();
    seen.sort();
    seen.dedup();
    assert!(
        seen.iter()
            .all(|n| n == "releases.win.json" || n == "Flare-0.0.2-full.nupkg"),
        "unexpected requests: {seen:?}"
    );
    assert_eq!(server.foreign_peers(), 0);
    drop(app);
}
