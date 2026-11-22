//! The agent helper, `flare-agent.jar`, on the emu host's side (Android plan PR A3, rule 1369).
//!
//! Per attached emulator: the jar is pushed (only to an `emulator-<port>` serial, never a real phone), started with
//! `app_process` through a held `adb shell` (so it ends with this host), and reached through `adb forward tcp:0
//! localabstract:flare-agent` on localhost with a random nonce on its command line, which its hello echoes. Any app in
//! the device could bind that socket name first, so only a helper that echoes the nonce this emu host process started
//! it with is reused; anything else is told to quit and the helpers are stopped by name, since a running helper blocks
//! a new one (Step 0 S4).
//!
//! The tree comes from the helper (about 5 ms warm). When a call fails, the helper is started again once; when that
//! fails too, the tree comes from `uiautomator dump` (about 2 s, "slow tree") until the next attach. The dump needs the
//! helper gone: both hold Android's single UI automation connection.
//!
//! Every function here BLOCKS (adb, the socket): the host calls them on blocking workers only.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

use flare_device::emu::agent::{self, Hello, Line, Tree};
use flare_device::geom::Size;
use flare_device::job::Fail;
use serde_json::{Value, json};

use crate::log::Log;
use crate::run::{command, run};

type Failed = (Fail, String);

/// How long the helper may take to say it is ready, to answer a hello, a tree and a set-text.
const START_LIMIT: Duration = Duration::from_secs(15);
const HELLO_LIMIT: Duration = Duration::from_secs(2);
const TREE_LIMIT: Duration = Duration::from_secs(10);
const TEXT_LIMIT: Duration = Duration::from_secs(5);
/// adb's own calls (forward, push, kill), and the dump.
const ADB_LIMIT: Duration = Duration::from_secs(20);
const DUMP_LIMIT: Duration = Duration::from_secs(30);
/// Failures (a start or a call) before the tree falls back to the dump for the rest of the attach: one restart.
const FAILURES_BEFORE_DUMP: u32 = 2;
/// The most lines of the helper's own output that reach the log per start.
const CHATTER_MAX: usize = 20;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Which way a tree came (`device_state` shows it in PR A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreePath {
    Helper,
    /// `uiautomator dump`: about 2 s a look ("slow tree").
    Dump,
}

impl TreePath {
    pub fn name(self) -> &'static str {
        match self {
            TreePath::Helper => "helper",
            TreePath::Dump => "dump",
        }
    }
}

/// The jar this build pushes, found once: beside the emu host (the installer, PR A6), in the app's `Resources`
/// (macOS), or in the repo for a debug build. Only a file with the hash this build expects counts.
pub fn jar() -> Option<&'static Path> {
    static JAR: OnceLock<Option<PathBuf>> = OnceLock::new();
    JAR.get_or_init(|| {
        candidates()
            .into_iter()
            .find(|p| sha256_of(p).as_deref() == Some(agent::jar_sha256()))
    })
    .as_deref()
}

/// Where the jar may be, in order.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        out.push(dir.join("flare-agent.jar"));
        out.push(dir.join("../Resources/flare-agent.jar"));
    }
    #[cfg(any(debug_assertions, test))]
    out.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../flare-device/assets/flare-agent.jar"
    )));
    out
}

/// A file's SHA-256 (hex), None when it cannot be read.
pub fn sha256_of(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(p).ok()?;
    Some(
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// The `agent_jar` fact in the hello: the jar's short hash, or `missing`.
pub fn jar_fact() -> String {
    match jar() {
        Some(_) => agent::jar_sha256().get(..12).unwrap_or("").to_string(),
        None => "missing".to_string(),
    }
}

/// The nonce of the helper this emu host process started on each emulator (by serial). Only a helper that echoes it
/// is reused; it never leaves this process.
fn started() -> &'static Mutex<HashMap<String, String>> {
    static STARTED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    STARTED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A fresh nonce for a helper's start: 128 bits from the OS.
fn new_nonce() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b)
        .map_err(|e| format!("no random nonce for the agent helper: {e}"))?;
    Ok(agent::nonce_from(b))
}

/// Why a call to the helper did not give a result.
#[derive(Debug)]
enum CallErr {
    /// The helper answered with an error: it is alive.
    Refused(String, String),
    /// No answer (closed, timed out, unreadable): the helper is gone or stuck.
    Broken(String),
}

/// An answer from the helper: its call's id, and the result or a code and why.
type Answer = (u64, Result<Value, (String, String)>);

/// One connection to the helper through the forward. A reader thread takes every line: window events go into the
/// counter, answers into a channel.
struct Conn {
    w: TcpStream,
    answers: mpsc::Receiver<Answer>,
    next: u64,
}

impl Conn {
    fn open(port: u16, changed: Arc<AtomicU64>) -> Result<Conn, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|e| format!("the forward on port {port}: {e}"))?;
        let _ = s.set_nodelay(true);
        let r = s.try_clone().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("emu-agent-read".to_string())
            .spawn(move || {
                for line in BufReader::new(r).lines() {
                    let Ok(line) = line else { break };
                    match agent::read_line(&line) {
                        Some(Line::Windows { count }) => changed.store(count, Ordering::Relaxed),
                        Some(Line::Answer { id, result }) => {
                            if tx.send((id, result)).is_err() {
                                break;
                            }
                        }
                        None => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Conn {
            w: s,
            answers: rx,
            next: 1,
        })
    }

    fn call(&mut self, op: &str, extra: Value, limit: Duration) -> Result<Value, CallErr> {
        let id = self.next;
        self.next += 1;
        let line = agent::request(id, op, extra) + "\n";
        self.w
            .write_all(line.as_bytes())
            .and_then(|()| self.w.flush())
            .map_err(|e| CallErr::Broken(format!("{op}: {e}")))?;
        let until = Instant::now() + limit;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.answers.recv_timeout(left) {
                Ok((got, r)) if got == id => {
                    return r.map_err(|(code, why)| CallErr::Refused(code, why));
                }
                // An answer to a call that timed out before: dropped.
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(CallErr::Broken(format!(
                        "{op}: no answer in {} s",
                        limit.as_secs()
                    )));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(CallErr::Broken(format!("{op}: the helper closed")));
                }
            }
        }
    }

    fn hello(&mut self) -> Result<Hello, CallErr> {
        let v = self.call("hello", Value::Null, HELLO_LIMIT)?;
        serde_json::from_value(v).map_err(|e| CallErr::Broken(format!("hello: {e}")))
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = self.w.shutdown(std::net::Shutdown::Both);
    }
}

#[derive(Default)]
struct State {
    conn: Option<Conn>,
    /// The local port of `adb forward`, kept for the attach.
    port: Option<u16>,
    /// The held `adb shell` the helper runs under; None for a reused helper (another Flare's), which is never told
    /// to quit.
    child: Option<Child>,
    failures: u32,
    /// After a failed restart: the dump only, until the next attach.
    dump_only: bool,
}

/// The helper of one attached emulator.
pub struct Helper {
    adb: PathBuf,
    serial: String,
    log: Log,
    /// The window changes the helper has counted (the "changed" counter while paused, PR A4).
    changed: Arc<AtomicU64>,
    st: Mutex<State>,
}

impl Helper {
    /// None when `serial` is not an emulator's (rule 1369: never a real phone in v1).
    pub fn new(adb: PathBuf, serial: String, log: Log) -> Option<Helper> {
        agent::serial_ok(&serial).then(|| Helper {
            adb,
            serial,
            log,
            changed: Arc::new(AtomicU64::new(0)),
            st: Mutex::new(State::default()),
        })
    }

    /// The window changes the helper has seen since it started.
    pub fn changed(&self) -> u64 {
        self.changed.load(Ordering::Relaxed)
    }

    fn adb(&self, args: &[&str], limit: Duration) -> Result<crate::run::Ran, String> {
        let mut c = command(&self.adb);
        c.arg("-s").arg(&self.serial).args(args);
        run(c, limit)
    }

    /// BLOCKING: starts the helper now, so the first look is quick (called after attach). A failure is logged and
    /// counted.
    pub fn warm(&self) {
        let mut st = lock(&self.st);
        if let Err(e) = self.ensure(&mut st) {
            self.log
                .line(&format!("{}: agent helper: {e}", self.serial));
        }
    }

    /// The connection, starting the helper when there is none. A failed start counts as a failure.
    fn ensure<'a>(&self, st: &'a mut State) -> Result<&'a mut Conn, String> {
        if st.dump_only {
            return Err(
                "the agent helper failed twice; the tree uses uiautomator dump".to_string(),
            );
        }
        if st.conn.is_none() {
            let started = Instant::now();
            match self.start(st) {
                Ok(how) => self.log.line(&format!(
                    "{}: agent helper {how} in {} ms",
                    self.serial,
                    started.elapsed().as_millis()
                )),
                Err(e) => {
                    self.failed(st);
                    return Err(e);
                }
            }
        }
        st.conn
            .as_mut()
            .ok_or_else(|| "no agent helper".to_string())
    }

    /// A start or a call failed: drop the helper, and after the restart failed too, use the dump.
    fn failed(&self, st: &mut State) {
        st.conn = None;
        if let Some(mut c) = st.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        st.failures += 1;
        if st.failures >= FAILURES_BEFORE_DUMP && !st.dump_only {
            st.dump_only = true;
            self.log.line(&format!(
                "{}: the agent helper failed {} times; the tree uses uiautomator dump (slow) until the next attach",
                self.serial, st.failures
            ));
        }
    }

    /// Reuses a helper of this build, or pushes and starts one. Says which.
    fn start(&self, st: &mut State) -> Result<&'static str, String> {
        let port = match st.port {
            Some(p) => p,
            None => {
                let r = self.adb(
                    &[
                        "forward",
                        "tcp:0",
                        &format!("localabstract:{}", agent::SOCKET),
                    ],
                    ADB_LIMIT,
                )?;
                let p = agent::forward_port(&r.stdout)
                    .ok_or_else(|| format!("adb forward said {:?}", r.stderr.trim()))?;
                st.port = Some(p);
                p
            }
        };
        // Something already answers on the socket. Any app in the device could have bound that name first, so it is
        // reused only when its hello echoes the nonce this emu host started a helper with on this device. Anything
        // else (an app, another Flare's helper, an old build) is told to quit and the helpers are stopped by name.
        let known = lock(started()).get(&self.serial).cloned();
        if let Ok(mut c) = Conn::open(port, Arc::clone(&self.changed)) {
            match c.hello() {
                Ok(h) if known.as_deref().is_some_and(|n| h.is_ours(n)) => {
                    st.conn = Some(c);
                    return Ok("reused");
                }
                Ok(h) => {
                    self.log.line(&format!(
                        "{}: something that is not this host's agent helper answers on its socket (protocol {}, {}): \
                         stopping it",
                        self.serial,
                        h.protocol,
                        h.jar_sha256.get(..12).unwrap_or("?")
                    ));
                    let _ = c.call("quit", Value::Null, HELLO_LIMIT);
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(_) => {}
            }
        }
        self.kill_leftovers();
        let nonce = new_nonce()?;
        let jar = jar().ok_or_else(|| {
            format!(
                "no flare-agent.jar with SHA-256 {} beside the emu host",
                agent::jar_sha256()
            )
        })?;
        let device_jar = agent::device_jar(agent::jar_sha256());
        // Jars of other builds go (a running helper keeps its loaded code).
        let _ = self.adb(&["shell", &agent::sweep_command(&device_jar)], ADB_LIMIT);
        let pushed = self.adb(&["push", &jar.to_string_lossy(), &device_jar], ADB_LIMIT)?;
        if !pushed.ok() {
            return Err(format!("adb push failed: {}", pushed.stderr.trim()));
        }
        let child = self.spawn(&device_jar, &nonce)?;
        st.child = Some(child);
        let mut c = Conn::open(port, Arc::clone(&self.changed))?;
        let h = c.hello().map_err(|e| format!("{e:?}"))?;
        if !h.is_ours(&nonce) {
            return Err(format!(
                "the agent helper that answered is not the one this host started ({})",
                h.jar_sha256.get(..12).unwrap_or("?")
            ));
        }
        lock(started()).insert(self.serial.clone(), nonce);
        self.log.line(&format!(
            "{}: agent helper on Android {} (API {})",
            self.serial, h.release, h.api
        ));
        st.conn = Some(c);
        Ok("started")
    }

    /// Ends any helper by its process name (an old build, or one stuck). Quiet when there is none.
    fn kill_leftovers(&self) {
        let _ = self.adb(
            &[
                "shell",
                &format!("kill $(pidof {}) 2>/dev/null; true", agent::PROCESS),
            ],
            ADB_LIMIT,
        );
    }

    /// Starts the helper under a held `adb shell` and waits for its `ready` line. Its later output goes to the log.
    fn spawn(&self, device_jar: &str, nonce: &str) -> Result<Child, String> {
        let mut c = command(&self.adb);
        c.arg("-s")
            .arg(&self.serial)
            .arg("shell")
            .arg(agent::start_command(device_jar, nonce))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| format!("adb could not start: {e}"))?;
        let (tx, rx) = mpsc::channel::<String>();
        let pipe = |r: Box<dyn std::io::Read + Send>, tx: mpsc::Sender<String>| {
            std::thread::spawn(move || {
                for line in BufReader::new(r).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
        };
        if let Some(o) = child.stdout.take() {
            pipe(Box::new(o), tx.clone());
        }
        if let Some(e) = child.stderr.take() {
            pipe(Box::new(e), tx);
        }
        let first = rx.recv_timeout(START_LIMIT);
        let ready = match first {
            Ok(l) => agent::ready_line(l.trim()),
            Err(_) => Err(format!(
                "the agent helper said nothing in {} s",
                START_LIMIT.as_secs()
            )),
        };
        if let Err(e) = ready {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        // What it says after `ready` (a crash's stack, say) goes to the log, a few lines at most.
        let log = self.log.clone();
        let serial = self.serial.clone();
        std::thread::spawn(move || {
            for (i, l) in rx.iter().enumerate() {
                if i < CHATTER_MAX {
                    log.line(&format!(
                        "{serial}: agent helper: {}",
                        l.chars().take(300).collect::<String>()
                    ));
                }
            }
        });
        Ok(child)
    }

    /// BLOCKING: the tree in the tree model, frames in dp: `{"tree", "path", "front_app", "changed"}`. From the
    /// helper (started again once when it fails), else from `uiautomator dump`.
    pub fn tree(&self, scale: f64, screen_dp: Size) -> Result<Value, Failed> {
        let mut st = lock(&self.st);
        while !st.dump_only {
            let r = match self.ensure(&mut st) {
                Ok(c) => c.call("tree", Value::Null, TREE_LIMIT),
                Err(_) => continue,
            };
            match r {
                Ok(v) => match serde_json::from_value::<Tree>(v) {
                    Ok(t) => {
                        // A good tree: an old hiccup no longer counts towards the dump.
                        st.failures = 0;
                        return Ok(self.answer(&t, scale, screen_dp, TreePath::Helper));
                    }
                    Err(e) => {
                        self.log.line(&format!(
                            "{}: an agent tree that does not read: {e}",
                            self.serial
                        ));
                        self.failed(&mut st);
                    }
                },
                // The helper is alive but cannot read the tree (a new Android version, a lost connection to the
                // UI automation): a failure like no answer, so the dump takes over after one restart (rule 1369).
                Err(CallErr::Refused(code, why)) => {
                    self.log.line(&format!(
                        "{}: agent helper refused the tree ({code}): {why}",
                        self.serial
                    ));
                    self.failed(&mut st);
                }
                Err(CallErr::Broken(e)) => {
                    self.log
                        .line(&format!("{}: agent helper: {e}", self.serial));
                    self.failed(&mut st);
                }
            }
        }
        let t = self.dump(&mut st).map_err(|e| (Fail::Unavailable, e))?;
        Ok(self.answer(&t, scale, screen_dp, TreePath::Dump))
    }

    fn answer(&self, t: &Tree, scale: f64, screen_dp: Size, path: TreePath) -> Value {
        json!({
            "tree": agent::to_ui_tree(t, scale, screen_dp),
            "path": path.name(),
            "front_app": t.front_app,
            "changed": self.changed(),
        })
    }

    /// `uiautomator dump` (about 2 s). Any helper is ended first: both need the one UI automation connection, and
    /// the dump fails quietly while a helper holds it (checked by hand, 2026-11-22).
    fn dump(&self, st: &mut State) -> Result<Tree, String> {
        st.conn = None;
        if let Some(mut c) = st.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.kill_leftovers();
        let r = self.adb(
            &["shell", "uiautomator", "dump", agent::DUMP_PATH],
            DUMP_LIMIT,
        )?;
        if !r.stdout.contains("dumped to") {
            return Err(format!(
                "uiautomator dump failed: {}",
                format!("{} {}", r.stdout.trim(), r.stderr.trim())
                    .chars()
                    .take(200)
                    .collect::<String>()
            ));
        }
        let xml = self.adb(&["exec-out", "cat", agent::DUMP_PATH], ADB_LIMIT)?;
        agent::parse_dump(&xml.stdout)
            .ok_or_else(|| "a uiautomator dump that does not read".to_string())
    }

    /// BLOCKING: types `text` at the cursor of the focused field through the helper (rule 1368, the step before the
    /// device's clipboard). Err when the helper cannot (no field, a password field, no helper): the caller pastes.
    pub fn insert_text(&self, text: &str) -> Result<(), String> {
        let mut st = lock(&self.st);
        let c = self.ensure(&mut st)?;
        match c.call(
            "set_text",
            json!({ "text": text, "mode": "insert" }),
            TEXT_LIMIT,
        ) {
            Ok(v) => {
                if v.get("matches") == Some(&Value::Bool(false)) {
                    self.log.line(&format!(
                        "{}: set-text: the field shows other text than was set",
                        self.serial
                    ));
                }
                Ok(())
            }
            Err(CallErr::Refused(_, why)) => Err(why),
            Err(CallErr::Broken(e)) => {
                self.failed(&mut st);
                Err(e)
            }
        }
    }

    /// BLOCKING: ends the helper and the forward (detach, or the emulator went).
    pub fn stop(&self) {
        let mut st = lock(&self.st);
        // Only a helper this host started is told to quit; a reused one (another Flare's) only loses this client
        // and ends by itself when its last client is gone.
        if st.child.is_some()
            && let Some(c) = st.conn.as_mut()
        {
            let _ = c.call("quit", Value::Null, HELLO_LIMIT);
        }
        st.conn = None;
        if let Some(mut c) = st.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(p) = st.port.take() {
            let _ = self.adb(&["forward", "--remove", &format!("tcp:{p}")], ADB_LIMIT);
        }
    }
}

/// `flare-emu-host --agent <serial> [--text <t>]`, by hand against a running emulator: starts the helper, times a
/// first tree and 20 warm ones, types `t` into the focused field when given, then the dump once for comparison, and
/// stops. JSON out.
pub fn check(serial: &str, text: Option<&str>) -> Result<Value, String> {
    let places = flare_device::emu::Places::from_env();
    let adb = match flare_device::emu::sdk::find(&places) {
        flare_device::emu::sdk::Found::Ready(s) | flare_device::emu::sdk::Found::TooOld(s) => {
            s.adb()
        }
        flare_device::emu::sdk::Found::Missing => return Err("no Android SDK".to_string()),
    };
    let h = Helper::new(adb, serial.to_string(), Log::open(None))
        .ok_or_else(|| format!("{serial} is not an emulator's serial"))?;
    let screen = Size::new(411.0, 914.0);
    let t0 = Instant::now();
    h.warm();
    let start_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut times = Vec::new();
    let mut first = Value::Null;
    for i in 0..21 {
        let t = Instant::now();
        let v = h.tree(2.625, screen).map_err(|(_, e)| e)?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        if i == 0 {
            first = v;
        }
    }
    let first_ms = times.remove(0);
    times.sort_by(f64::total_cmp);
    let typed = text.map(|t| h.insert_text(t));
    let changed = h.changed();
    // The dump, for comparison (it stops the helper first).
    let t = Instant::now();
    let dumped = {
        let mut st = lock(&h.st);
        h.dump(&mut st).map(|d| d.nodes)
    };
    let dump_ms = t.elapsed().as_secs_f64() * 1000.0;
    h.stop();
    let refs = first
        .get("tree")
        .and_then(|t| serde_json::from_value::<flare_device::tree::UiNode>(t.clone()).ok())
        .map(|t| flare_device::tree::filter_tree(&t, screen).refs.len());
    Ok(json!({
        "start_ms": start_ms,
        "first_tree_ms": first_ms,
        "tree_p50_ms": times[times.len() / 2],
        "tree_max_ms": times.last(),
        "path": first["path"],
        "front_app": first["front_app"],
        "refs": refs,
        "window_changes": changed,
        "set_text": typed.map(|r| r.map_or_else(|e| e, |()| "ok".to_string())),
        "dump_ms": dump_ms,
        "dump_nodes": dumped.map_or_else(Value::from, Value::from),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A stand-in helper on a local port: answers hello and tree, refuses set_text, sends a window event, and goes
    /// silent on `stuck`.
    fn fake_helper() -> u16 {
        fake_helper_with(false, "")
    }

    /// The same, refusing every tree when `refuse_tree` (a helper that cannot read the screen), and echoing `nonce`.
    fn fake_helper_with(refuse_tree: bool, nonce: &'static str) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { break };
                std::thread::spawn(move || {
                    let mut w = s.try_clone().unwrap();
                    for line in BufReader::new(s).lines().map_while(Result::ok) {
                        let v: Value = serde_json::from_str(&line).unwrap();
                        let id = v["id"].as_u64().unwrap();
                        let out = match v["op"].as_str().unwrap() {
                            "hello" => json!({"id": id, "ok": {
                                "agent": "flare-agent", "protocol": agent::PROTOCOL,
                                "jar_sha256": agent::jar_sha256(), "api": 37, "release": "17",
                                "nonce": nonce}}),
                            "tree" if refuse_tree => {
                                json!({"id": id, "code": "unavailable", "err": "NullPointerException"})
                            }
                            "tree" => {
                                writeln!(w, r#"{{"event":"windows","count":42}}"#).unwrap();
                                json!({"id": id, "ok": {"windows": [], "front_app": "a.b", "nodes": 0}})
                            }
                            "stuck" => continue,
                            _ => {
                                json!({"id": id, "code": "not_found", "err": "no text field has the focus"})
                            }
                        };
                        writeln!(w, "{out}").unwrap();
                    }
                });
            }
        });
        port
    }

    #[test]
    fn calls_get_their_answers_and_events_count() {
        let changed = Arc::new(AtomicU64::new(0));
        let mut c = Conn::open(fake_helper(), Arc::clone(&changed)).unwrap();
        assert!(c.hello().unwrap().is_this_build());
        let t = c.call("tree", Value::Null, Duration::from_secs(5)).unwrap();
        assert_eq!(t["front_app"], "a.b");
        assert_eq!(changed.load(Ordering::Relaxed), 42);
        match c.call("set_text", json!({"text": "x"}), Duration::from_secs(5)) {
            Err(CallErr::Refused(code, why)) => {
                assert_eq!(code, "not_found");
                assert!(why.contains("focus"));
            }
            other => panic!("{other:?}"),
        }
        let started = Instant::now();
        match c.call("stuck", Value::Null, Duration::from_millis(200)) {
            Err(CallErr::Broken(e)) => assert!(e.contains("no answer"), "{e}"),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        // The connection still works after a timed-out call.
        assert!(c.hello().is_ok());
    }

    #[test]
    fn a_closed_helper_is_broken_not_refused() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Accept, then close at once: what adb's forward does when nothing listens in the device.
            let _ = l.accept();
        });
        let mut c = Conn::open(port, Arc::new(AtomicU64::new(0))).unwrap();
        assert!(matches!(c.hello(), Err(CallErr::Broken(_))));
    }

    /// A helper connected to a stand-in (no adb on this path: a restart fails).
    fn helper_on(port: u16) -> Helper {
        let h = Helper::new(
            "/no/such/adb".into(),
            "emulator-5554".into(),
            Log::open(None),
        )
        .unwrap();
        lock(&h.st).conn = Some(Conn::open(port, Arc::clone(&h.changed)).unwrap());
        h
    }

    /// A good tree answers from the helper and clears an old failure; a helper that refuses every tree is a failure
    /// too, so after one failed restart the attach falls back to the dump (rule 1369).
    #[test]
    fn a_good_tree_clears_failures_and_a_refused_tree_falls_back() {
        let h = helper_on(fake_helper());
        lock(&h.st).failures = 1;
        let v = h.tree(2.625, Size::new(411.0, 914.0)).unwrap();
        assert_eq!(v["path"], "helper");
        assert_eq!(v["front_app"], "a.b");
        assert_eq!(v["changed"], 42);
        assert_eq!(v["tree"]["role"], "AXApplication");
        assert_eq!(lock(&h.st).failures, 0);
        h.stop();

        let h = helper_on(fake_helper_with(true, ""));
        assert!(h.tree(2.625, Size::new(411.0, 914.0)).is_err());
        let st = lock(&h.st);
        assert!(st.dump_only, "refused, then the restart failed: the dump");
        assert_eq!(st.failures, FAILURES_BEFORE_DUMP);
    }

    /// Security: something on the helper's socket that claims this build but does not echo the nonce this host
    /// started its helper with (an app that bound the name first) is never reused; the host starts its own instead
    /// (which fails here: no adb). One that echoes the nonce is reused.
    #[test]
    fn only_a_helper_that_echoes_this_hosts_nonce_is_reused() {
        const NONCE: &str = "0123456789abcdef0123456789abcdef";
        let impostor = Helper::new(
            "/no/such/adb".into(),
            "emulator-5596".into(),
            Log::open(None),
        )
        .unwrap();
        lock(started()).insert("emulator-5596".into(), NONCE.into());
        let mut st = State {
            port: Some(fake_helper_with(false, "ffffffffffffffffffffffffffffffff")),
            ..State::default()
        };
        assert!(
            impostor.start(&mut st).is_err(),
            "not reused, and no adb to start one"
        );
        assert!(st.conn.is_none());

        let ours = Helper::new(
            "/no/such/adb".into(),
            "emulator-5598".into(),
            Log::open(None),
        )
        .unwrap();
        let mut st = State {
            port: Some(fake_helper_with(false, NONCE)),
            ..State::default()
        };
        assert!(
            ours.start(&mut st).is_err(),
            "no nonce known for this device yet"
        );
        lock(started()).insert("emulator-5598".into(), NONCE.into());
        assert_eq!(ours.start(&mut st), Ok("reused"));
        assert!(st.conn.is_some());

        let n = new_nonce().unwrap();
        assert!(agent::nonce_ok(&n));
        assert_ne!(n, new_nonce().unwrap());
    }

    /// Rule 1369: a helper only for an emulator's serial.
    #[test]
    fn a_helper_is_made_only_for_an_emulator() {
        let log = Log::open(None);
        assert!(Helper::new("adb".into(), "emulator-5554".into(), log.clone()).is_some());
        assert!(Helper::new("adb".into(), "R58M12ABCDE".into(), log.clone()).is_none());
        assert!(Helper::new("adb".into(), "192.168.1.5:5555".into(), log).is_none());
    }

    /// The jar this build takes is found (the repo's, in a test build) and its hash is the expected one.
    #[test]
    fn the_jar_is_found_with_its_hash() {
        let p = jar().expect("the repo's jar, in a debug build");
        assert_eq!(sha256_of(p).as_deref(), Some(agent::jar_sha256()));
        assert_eq!(jar_fact(), &agent::jar_sha256()[..12]);
        assert_eq!(sha256_of(Path::new("/no/such/flare-agent.jar")), None);
    }

    /// With no adb and no device, a tree still answers (an error, never a hang), and the helper falls to the dump
    /// after one restart.
    #[test]
    fn a_helper_that_cannot_start_falls_back_and_answers() {
        let h = Helper::new(
            "/no/such/adb".into(),
            "emulator-5554".into(),
            Log::open(None),
        )
        .unwrap();
        let started = Instant::now();
        let e = h.tree(2.625, Size::new(411.0, 914.0)).unwrap_err();
        assert_eq!(e.0, Fail::Unavailable);
        assert!(lock(&h.st).dump_only, "two failures: the dump from now on");
        assert_eq!(lock(&h.st).failures, FAILURES_BEFORE_DUMP);
        assert!(h.insert_text("x").is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        h.stop();
    }
}
