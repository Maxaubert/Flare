//! Flare's side of the emu host: `EmuSource`, the Android [`DeviceSource`] (Android plan PR A2; not wired until A4).
//!
//! Like `SimSource` (`client.rs`), it starts `flare-emu-host` guarded so it dies with Flare (macOS:
//! `flare_proc::guard`; Windows: a kill-on-close job, `flare_proc::job`), talks to it in JSON lines over its stdin and
//! stdout, restarts it at most [`MAX_RESTARTS`] times in [`RESTART_WINDOW`] (then the source is broken for the
//! session and every request answers `unavailable` with the reason), and never blocks the caller (rule 34): every
//! method writes to a channel or starts a worker, and every answer comes back as a [`SourceEvent`].
//!
//! What only Flare does:
//! - **The frame region** (rule 1363): when the emu host says a device's screen (its `frames` line), Flare makes a
//!   user-only file for it in the `flare` folder of the discovery folder, sized for every rotation, and hands its URL
//!   to the emu host (`region`). The emulator writes frames into it; each frame's facts come back as a `region` line
//!   ([`SourceEvent::Region`]). The file goes on detach, and files a gone Flare left behind go when the source starts.
//! - **Booting** (rule 1366): Flare starts the emulator itself, on a worker, detached (its own session or process
//!   group, outside any job, no window), with the command line of rule 1362; the emu host then waits for it to boot.
//!   The emu host is guarded, and a guard ends every process its program started, so an emulator started there would
//!   die with Flare instead of shutting itself down cleanly after its idle timeout.
//! - **Which emulators Flare booted** this session (`booted_by_flare` in the device list), for the quit rule.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::Restarts;
use crate::emu::region::{self, Region};
use crate::emu::{Places, avds, boot, discovery, sdk};
use crate::job::Fail;
use crate::source::{
    Answer, Button, DeviceInfo, DeviceSource, Key, ReqId, SourceError, SourceEvent, Touch,
};
use crate::target::Target;
use crate::wire::{Control, Hello, HostKind, HostLine, PROTOCOL, Request, from_line, to_line};

pub use crate::client::{MAX_RESTARTS, RESTART_WINDOW};

/// The broken line's reason when the emu host kept stopping.
pub const KEPT_STOPPING: &str = "the emu host stopped four times in five minutes";

/// Called on the source's own threads after each event is queued (Flare posts one coalesced wake). Never blocks.
pub type OnEvent = Arc<dyn Fn() + Send + Sync>;

/// Starts the emulator for a target (tests put a stand-in here). Runs on a worker.
pub type Launcher = Arc<dyn Fn(&Target) -> Result<(), String> + Send + Sync>;

/// One frame the emulator wrote into a device's region (rule 1364): the record's facts and the region file. Handed to
/// Flare's frame thread straight from the emu host's reader, so the copy never waits for the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionFrame {
    pub target: Target,
    pub seq: u64,
    pub t_us: u64,
    /// The frame's size in pixels, already turned (a landscape frame is wider than high).
    pub w: u32,
    pub h: u32,
    pub rotation: crate::geom::Orientation,
    /// The region file the emulator wrote it into.
    pub path: PathBuf,
}

/// Takes each [`RegionFrame`] on the emu host's reader thread. Never blocks.
pub type OnRegion = Arc<dyn Fn(RegionFrame) + Send + Sync>;

/// How Flare starts its emu host.
#[derive(Clone)]
pub struct EmuConfig {
    /// `flare-emu-host` (next to Flare's exe).
    pub exe: PathBuf,
    /// `<profile>/logs/emu-host.log`.
    pub log: PathBuf,
    /// Where each booted emulator's own output goes (`emulator-<avd>.log`); None: nowhere.
    pub logs_dir: Option<PathBuf>,
    pub places: Places,
    /// Arguments before the emu host's own (tests: the test binary runs one test as the fake emu host).
    pub pre_args: Vec<String>,
    /// More environment for the emu host (tests).
    pub env: Vec<(String, String)>,
    pub restart_delay: Duration,
    pub max_restarts: usize,
    /// Told after each event (Flare's wake).
    pub on_event: Option<OnEvent>,
    /// The region folder; None: the `flare` folder of the discovery folder (tests give their own).
    pub region_dir: Option<PathBuf>,
    /// Tests: instead of starting a real emulator.
    pub launcher: Option<Launcher>,
    /// Flare's frame thread (rule 1364): each frame record with its region, as soon as it is read.
    pub on_region: Option<OnRegion>,
}

impl EmuConfig {
    pub fn new(exe: PathBuf, log: PathBuf, places: Places) -> EmuConfig {
        EmuConfig {
            exe,
            log,
            logs_dir: None,
            places,
            pre_args: Vec::new(),
            env: Vec::new(),
            restart_delay: Duration::from_millis(500),
            max_restarts: MAX_RESTARTS,
            on_event: None,
            region_dir: None,
            launcher: None,
            on_region: None,
        }
    }

    fn region_dir(&self) -> Option<PathBuf> {
        self.region_dir
            .clone()
            .or_else(|| self.places.discovery_dir().map(|d| region::dir(&d)))
    }
}

/// Whether Flare takes this emu host: an emu host (not a sim host) speaking Flare's protocol. Err is the reason.
pub fn check_hello(h: &Hello) -> Result<(), String> {
    if h.host != HostKind::Emu {
        return Err("the emu host said it is a sim host (reinstall Flare)".to_string());
    }
    if h.protocol != PROTOCOL {
        return Err(format!(
            "the emu host speaks protocol {}, Flare {PROTOCOL} (reinstall Flare)",
            h.protocol
        ));
    }
    Ok(())
}

/// What an answer is read as.
#[derive(Clone, Copy, Debug)]
enum Kind {
    List,
    Done,
    Png,
    Tree,
}

#[derive(Clone, Debug)]
struct Att {
    target: Target,
    paused: bool,
}

#[derive(Default)]
struct State {
    hello: Option<Hello>,
    broken: Option<String>,
    pending: HashMap<ReqId, Kind>,
    /// Lines waiting for an emu host (between restarts).
    queued: Vec<String>,
    attached: Vec<Att>,
    starts: u32,
    /// Emulators Flare booted this session.
    booted: HashSet<Target>,
    /// Emulators Flare asked to shut down (their end is no failed boot).
    shutdown_asked: HashSet<Target>,
    regions: HashMap<Target, Region>,
    regions_made: u64,
}

#[cfg(target_os = "macos")]
type HostGuard = flare_proc::guard::Guard;
#[cfg(windows)]
type HostGuard = flare_proc::job::Guard;
#[cfg(not(any(windows, target_os = "macos")))]
type HostGuard = ();

fn release(g: &mut HostGuard) {
    #[cfg(any(windows, target_os = "macos"))]
    g.release();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = g;
}

struct Shared {
    cfg: EmuConfig,
    tx: Mutex<Sender<SourceEvent>>,
    state: Mutex<State>,
    writer: Mutex<Option<Sender<String>>>,
    guard: Mutex<Option<HostGuard>>,
    stop: AtomicBool,
    internal_id: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    fn event(&self, e: SourceEvent) {
        let _ = lock(&self.tx).send(e);
        if let Some(wake) = &self.cfg.on_event {
            wake();
        }
    }

    fn answer(&self, id: ReqId, result: Result<Answer, SourceError>) {
        self.event(SourceEvent::Answer { id, result });
    }

    fn fail(&self, id: ReqId, fail: Fail, message: impl Into<String>) {
        self.answer(
            id,
            Err(SourceError {
                fail,
                message: message.into(),
            }),
        );
    }

    fn broken(&self) -> Option<String> {
        lock(&self.state).broken.clone()
    }

    fn internal(&self) -> ReqId {
        u64::MAX / 2 + self.internal_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Writes a control line to the running emu host, or keeps it for the next one.
    fn send(&self, req: Request, id: ReqId, kind: Option<Kind>) {
        let line = to_line(&Control { id, req });
        let mut st = lock(&self.state);
        if let Some(k) = kind {
            st.pending.insert(id, k);
        }
        match lock(&self.writer).as_ref() {
            Some(w) if w.send(line.clone()).is_ok() => {}
            _ => st.queued.push(line),
        }
    }

    /// Broken for the session: say why once; every request from now answers `unavailable`.
    fn go_broken(&self, why: String) {
        {
            let mut st = lock(&self.state);
            if st.broken.is_some() {
                return;
            }
            st.broken = Some(why.clone());
            for (id, _) in st.pending.drain() {
                self.fail(
                    id,
                    Fail::Unavailable,
                    format!("Android devices unavailable: {why}."),
                );
            }
            st.queued.clear();
            st.regions.clear();
        }
        log::warn!("device: the emu host is broken for the session: {why}");
        self.event(SourceEvent::Broken(why));
        if let Some(mut g) = lock(&self.guard).take() {
            release(&mut g);
        }
        *lock(&self.writer) = None;
    }

    /// A device's screen is known: make (or keep) its region and hand it to the emu host. On a worker.
    fn give_region(self: &Arc<Self>, target: Target, w: u32, h: u32) {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            let need = region::bytes_for(w, h);
            let have = lock(&me.state)
                .regions
                .get(&target)
                .filter(|r| r.bytes() >= need)
                .map(|r| (r.url(), r.bytes()));
            let (url, bytes) = match have {
                Some(x) => x,
                None => {
                    let Some(dir) = me.cfg.region_dir() else {
                        log::warn!("device: {target}: no discovery folder for a frame region");
                        return;
                    };
                    let n = {
                        let mut st = lock(&me.state);
                        st.regions_made += 1;
                        st.regions_made
                    };
                    match Region::create(&dir, std::process::id(), n, need) {
                        Ok(r) => {
                            let x = (r.url(), r.bytes());
                            let mut st = lock(&me.state);
                            if !st.attached.iter().any(|a| a.target == target) {
                                return; // detached meanwhile: the new file goes with `r`
                            }
                            st.regions.insert(target.clone(), r);
                            x
                        }
                        Err(e) => {
                            log::warn!("device: {target}: no frame region: {e}");
                            return;
                        }
                    }
                }
            };
            let id = me.internal();
            me.send(Request::Region { target, url, bytes }, id, None);
        });
    }

    /// One line from the emu host.
    fn on_line(self: &Arc<Self>, line: &str) {
        let Some(l) = from_line::<HostLine>(line) else {
            if !line.trim().is_empty() {
                log::debug!(
                    "device: emu host said {:?}",
                    line.chars().take(200).collect::<String>()
                );
            }
            return;
        };
        match l {
            HostLine::Hello(h) => match check_hello(&h) {
                Ok(()) => {
                    log::info!("device: emu host {} {:?}", h.version, h.facts);
                    lock(&self.state).hello = Some(h);
                }
                Err(why) => {
                    lock(&self.state).hello = Some(h);
                    self.go_broken(why);
                }
            },
            HostLine::Ok { id, result } => {
                let kind = lock(&self.state).pending.remove(&id);
                let answer = match kind {
                    Some(Kind::List) => {
                        let mut list: Vec<DeviceInfo> = serde_json::from_value(
                            result.get("devices").cloned().unwrap_or(Value::Null),
                        )
                        .unwrap_or_default();
                        let booted = lock(&self.state).booted.clone();
                        for d in &mut list {
                            d.booted_by_flare = booted.contains(&d.target);
                        }
                        Answer::Devices(list)
                    }
                    Some(Kind::Done) => Answer::Done,
                    Some(Kind::Png) => match crate::wire::png_of(&result) {
                        Some(p) => Answer::Png(p),
                        None => {
                            self.fail(id, Fail::Unavailable, "the emu host sent no PNG");
                            return;
                        }
                    },
                    Some(Kind::Tree) => match crate::wire::tree_of(&result) {
                        Some(t) => Answer::Tree(t),
                        None => {
                            self.fail(id, Fail::Unavailable, "the emu host sent no UI tree");
                            return;
                        }
                    },
                    None => return,
                };
                self.answer(id, Ok(answer));
            }
            HostLine::Err { id, code, message } => {
                if lock(&self.state).pending.remove(&id).is_some() {
                    let fail = Fail::from_code(&code).unwrap_or(Fail::Unavailable);
                    self.fail(id, fail, message);
                }
            }
            HostLine::Frames {
                target,
                device,
                ring: _,
                screen,
            } => {
                let (w, h) = (screen.size_px.w as u32, screen.size_px.h as u32);
                self.event(SourceEvent::Frames {
                    target: target.clone(),
                    ring: device,
                    screen,
                });
                self.give_region(target, w, h);
            }
            HostLine::Orientation {
                target,
                orientation,
            } => self.event(SourceEvent::Orientation {
                target,
                orientation,
            }),
            HostLine::State { target, state } => {
                if state == crate::source::DeviceState::Shutdown {
                    lock(&self.state).booted.remove(&target);
                }
                self.event(SourceEvent::State { target, state });
            }
            HostLine::Input { target, state } => self.event(SourceEvent::Input { target, state }),
            HostLine::Region {
                target,
                seq,
                t_us,
                w,
                h,
                rotation,
            } => {
                // The frame thread copies it out of the region at once (rule 1364); the UI hears of it too.
                if let Some(on) = &self.cfg.on_region {
                    let path = lock(&self.state)
                        .regions
                        .get(&target)
                        .map(|r| r.path().to_path_buf());
                    if let Some(path) = path {
                        on(RegionFrame {
                            target: target.clone(),
                            seq,
                            t_us,
                            w,
                            h,
                            rotation,
                            path,
                        });
                    }
                }
                self.event(SourceEvent::Region {
                    target,
                    seq,
                    t_us,
                    size_px: crate::geom::Size::new(f64::from(w), f64::from(h)),
                    rotation,
                });
            }
            HostLine::Broken { why } => self.go_broken(why),
            // Android setup's progress and a first boot's "Preparing" (PR A2b): shown by the panel from PR A4.
            HostLine::Setup { event } => log::debug!("emu host setup: {event:?}"),
        }
    }
}

/// The emu host's command: its own arguments after the test's, no console window on Windows.
fn host_command(sh: &Shared) -> Command {
    let cfg = &sh.cfg;
    let mut cmd = Command::new(&cfg.exe); // sync-ok: builds the command; spawned on the supervisor thread
    cmd.args(&cfg.pre_args);
    if lock(&sh.state).starts > 0 {
        cmd.arg("--restarted");
    }
    cmd.arg("--log")
        .arg(&cfg.log)
        .envs(cfg.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flare_proc::job::NO_WINDOW);
    }
    cmd
}

/// Starts the emu host once, guarded, with its writer and stderr reader.
fn spawn(sh: &Arc<Shared>) -> std::io::Result<Child> {
    let mut cmd = host_command(sh);
    #[cfg(target_os = "macos")]
    let mut child = {
        let pending = flare_proc::guard::guard_command(&mut cmd)?;
        crate::client::mac::no_inherited_descriptors(&mut cmd);
        let child = cmd.spawn()?; // sync-ok: the supervisor thread
        *lock(&sh.guard) = Some(pending.started());
        child
    };
    #[cfg(windows)]
    let mut child = {
        let child = cmd.spawn()?; // sync-ok: the supervisor thread
        match flare_proc::job::guard_child(&child) {
            Ok(g) => *lock(&sh.guard) = Some(g),
            Err(e) => log::warn!("device: the emu host runs unguarded: {e}"),
        }
        child
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut child = cmd.spawn()?; // sync-ok: the supervisor thread
    let (wtx, wrx) = channel::<String>();
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            for line in wrx {
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            for l in BufReader::new(err).lines().map_while(Result::ok) {
                log::info!("emu host: {l}");
            }
        });
    }
    let queued = {
        let mut st = lock(&sh.state);
        st.starts += 1;
        std::mem::take(&mut st.queued)
    };
    for l in queued {
        let _ = wtx.send(l);
    }
    *lock(&sh.writer) = Some(wtx);
    Ok(child)
}

/// The supervisor: runs the emu host, reads its lines, restarts it, gives up to broken.
fn supervise(sh: Arc<Shared>) {
    let mut restarts = Restarts::new(sh.cfg.max_restarts, RESTART_WINDOW);
    let mut first = true;
    loop {
        if sh.stop.load(Ordering::Relaxed) || sh.broken().is_some() {
            return;
        }
        let mut child = match spawn(&sh) {
            Ok(c) => c,
            Err(e) => {
                sh.go_broken(format!("the emu host could not start: {e}"));
                return;
            }
        };
        if !first {
            // Every open panel again: attach (the region comes back with its frames line), and pause what was paused.
            let attached = lock(&sh.state).attached.clone();
            for a in attached {
                sh.send(
                    Request::Attach {
                        target: a.target.clone(),
                    },
                    sh.internal(),
                    None,
                );
                if a.paused {
                    sh.send(Request::Pause { target: a.target }, sh.internal(), None);
                }
            }
        }
        first = false;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                sh.on_line(&line);
            }
        }
        let status = child.wait(); // sync-ok: its stdout closed, so it ended (or ends now)
        *lock(&sh.writer) = None;
        if let Some(mut g) = lock(&sh.guard).take() {
            release(&mut g);
        }
        let pending: Vec<ReqId> = lock(&sh.state).pending.drain().map(|(id, _)| id).collect();
        if sh.stop.load(Ordering::Relaxed) || sh.broken().is_some() {
            for id in pending {
                sh.fail(id, Fail::Unavailable, "the emu host ended");
            }
            return;
        }
        for id in pending {
            sh.fail(id, Fail::Retry, "the emu host restarted");
        }
        log::warn!("device: the emu host ended ({status:?})");
        if !restarts.ended(Instant::now()) {
            sh.go_broken(KEPT_STOPPING.to_string());
            return;
        }
        std::thread::sleep(sh.cfg.restart_delay); // sync-ok: the supervisor's pause before a restart
    }
}

/// BLOCKING (reads the SDK and the AVD folders, binds ports, starts a process): boots `target` with Flare's command
/// line (rule 1362), detached (rule 1366); `cold` adds `-no-snapshot-load` ([`boot::cold`]). The emulator's output
/// goes to `emulator-<avd>.log` in `logs_dir`. The caller reaps the child.
pub fn launch_emulator(
    places: &Places,
    logs_dir: Option<&Path>,
    target: &Target,
    cold: bool,
) -> Result<Child, String> {
    let sdk = match sdk::find(places) {
        sdk::Found::Ready(s) => s,
        sdk::Found::TooOld(s) => {
            return Err(format!(
                "the Android emulator {} is older than {}",
                s.emulator_version,
                sdk::floor()
            ));
        }
        sdk::Found::Missing => return Err("no Android emulator on this computer".to_string()),
    };
    let avd = avds::list(&avds::homes(places))
        .into_iter()
        .find(|a| a.name == target.id())
        .ok_or_else(|| format!("No virtual device named {} on this computer.", target.id()))?;
    let running = places
        .discovery_dir()
        .map(|d| discovery::read_all(&d))
        .unwrap_or_default();
    let taken: Vec<u16> = running
        .iter()
        .flat_map(|r| [r.console_port, r.adb_port])
        .flatten()
        .collect();
    let console = boot::console_port(&taken, boot::port_busy).ok_or("no free emulator port")?;
    let grpc = boot::free_port().ok_or("no free port")?;
    let args = boot::args(&avd.name, grpc, console);
    let args = if cold { boot::cold(args) } else { args };
    let log = || {
        logs_dir.and_then(|d| {
            let _ = std::fs::create_dir_all(d); // sync-ok: the boot worker
            std::fs::File::create(d.join(format!("emulator-{}.log", avd.name))).ok() // sync-ok: the boot worker
        })
    };
    let mut cmd = boot::command(&sdk.emulator(), &sdk.root, &avd.home, &args, log(), true)?;
    let child = match cmd.spawn() {
        Ok(c) => c,
        // A parent job that forbids breaking away (Windows): start it inside, it still outlives Flare's own process.
        Err(_) if cfg!(windows) => {
            boot::command(&sdk.emulator(), &sdk.root, &avd.home, &args, log(), false)?
                .spawn()
                .map_err(|e| format!("the emulator did not start: {e}"))?
        }
        Err(e) => return Err(format!("the emulator did not start: {e}")),
    };
    log::info!(
        "device: {target}: started the emulator {} (pid {}): {}",
        sdk.emulator_version,
        child.id(),
        args.join(" ")
    );
    Ok(child)
}

/// Waits for an emulator Flare started (so it never lingers as a zombie on macOS). One that ends within
/// [`boot::ENDED_AT_ONCE_S`] while nobody asked it to shut down is started once more as a cold boot (see
/// [`boot::cold`]); the emu host's boot wait sees the new one by its AVD name.
fn reap(sh: Arc<Shared>, target: Target, child: Child, cold: bool) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut child = child;
        let s = child.wait(); // sync-ok: the reaper's own thread
        log::info!("device: {target}: the emulator ended ({s:?})");
        let asked = lock(&sh.state).shutdown_asked.remove(&target);
        if cold
            || asked
            || sh.stop.load(Ordering::Relaxed)
            || started.elapsed() >= Duration::from_secs(boot::ENDED_AT_ONCE_S)
        {
            return;
        }
        log::warn!("device: {target}: the emulator ended at once; a cold boot");
        match launch_emulator(&sh.cfg.places, sh.cfg.logs_dir.as_deref(), &target, true) {
            Ok(c) => reap(sh, target, c, true),
            Err(e) => log::warn!("device: {target}: no cold boot: {e}"),
        }
    });
}

/// The device source behind the Android emulator.
pub struct EmuSource {
    sh: Arc<Shared>,
    rx: Receiver<SourceEvent>,
    next_id: ReqId,
}

impl EmuSource {
    /// Starts the emu host (on its own threads; returns at once), and deletes the frame regions a gone Flare left.
    pub fn start(cfg: EmuConfig) -> EmuSource {
        let (tx, rx) = channel();
        if let Some(dir) = cfg.region_dir() {
            std::thread::spawn(move || {
                let n = region::clean_stale(&dir, std::process::id(), pid_alive);
                if n > 0 {
                    log::info!("device: deleted {n} frame regions a gone Flare left");
                }
            });
        }
        let sh = Arc::new(Shared {
            cfg,
            tx: Mutex::new(tx),
            state: Mutex::new(State::default()),
            writer: Mutex::new(None),
            guard: Mutex::new(None),
            stop: AtomicBool::new(false),
            internal_id: AtomicU64::new(0),
        });
        let s = Arc::clone(&sh);
        std::thread::spawn(move || supervise(s));
        EmuSource { sh, rx, next_id: 1 }
    }

    fn id(&mut self) -> ReqId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Broken for the session, and why.
    pub fn broken(&self) -> Option<String> {
        self.sh.broken()
    }

    /// How many times the emu host was started.
    pub fn starts(&self) -> u32 {
        lock(&self.sh.state).starts
    }

    /// The emulators Flare booted this session (the quit rule, A4).
    pub fn booted(&self) -> Vec<Target> {
        let mut v: Vec<Target> = lock(&self.sh.state).booted.iter().cloned().collect();
        v.sort();
        v
    }

    /// The frame region of an attached target (A4 maps it), None before the emu host said its screen.
    pub fn region_path(&self, target: &Target) -> Option<PathBuf> {
        lock(&self.sh.state)
            .regions
            .get(target)
            .map(|r| r.path().to_path_buf())
    }

    /// Sends `req`, or answers `unavailable` at once when broken.
    fn request(&mut self, req: Request, kind: Kind) -> ReqId {
        let id = self.id();
        match self.sh.broken() {
            Some(why) => self.sh.fail(
                id,
                Fail::Unavailable,
                format!("Android devices unavailable: {why}."),
            ),
            None => self.sh.send(req, id, Some(kind)),
        }
        id
    }

    /// Sends `req` with no answer expected (input), dropped when broken.
    fn tell(&self, req: Request) {
        if self.sh.broken().is_none() {
            let id = self.sh.internal();
            self.sh.send(req, id, None);
        }
    }
}

/// Whether a process still runs (for the stale regions).
fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    return flare_proc::job::pid_alive(pid);
    #[cfg(target_os = "macos")]
    return flare_proc::alive(pid);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = pid;
        true
    }
}

impl Drop for EmuSource {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Relaxed);
        let id = self.sh.internal();
        self.sh.send(Request::Quit, id, None);
        *lock(&self.sh.writer) = None;
        if let Some(mut g) = lock(&self.sh.guard).take() {
            release(&mut g);
        }
        lock(&self.sh.state).regions.clear();
    }
}

impl DeviceSource for EmuSource {
    fn list(&mut self) -> ReqId {
        self.request(Request::List, Kind::List)
    }

    fn boot(&mut self, target: &Target) -> ReqId {
        let id = self.id();
        if let Some(why) = self.sh.broken() {
            self.sh.fail(
                id,
                Fail::Unavailable,
                format!("Android devices unavailable: {why}."),
            );
            return id;
        }
        let sh = Arc::clone(&self.sh);
        let t = target.clone();
        std::thread::spawn(move || {
            let running = sh
                .cfg
                .places
                .discovery_dir()
                .map(|d| discovery::read_all(&d))
                .unwrap_or_default();
            if discovery::of_avd(&running, t.id()).is_none() {
                lock(&sh.state).shutdown_asked.remove(&t);
                let started = match &sh.cfg.launcher {
                    Some(l) => l(&t),
                    None => launch_emulator(&sh.cfg.places, sh.cfg.logs_dir.as_deref(), &t, false)
                        .map(|c| reap(Arc::clone(&sh), t.clone(), c, false)),
                };
                if let Err(e) = started {
                    sh.fail(id, Fail::Unavailable, e);
                    return;
                }
                lock(&sh.state).booted.insert(t.clone());
            }
            sh.send(Request::Boot { target: t }, id, Some(Kind::Done));
        });
        id
    }

    fn shutdown(&mut self, target: &Target) -> ReqId {
        lock(&self.sh.state).shutdown_asked.insert(target.clone());
        self.request(
            Request::Shutdown {
                target: target.clone(),
            },
            Kind::Done,
        )
    }

    fn attach(&mut self, target: &Target) -> ReqId {
        {
            let mut st = lock(&self.sh.state);
            if !st.attached.iter().any(|a| &a.target == target) {
                st.attached.push(Att {
                    target: target.clone(),
                    paused: false,
                });
            }
        }
        self.request(
            Request::Attach {
                target: target.clone(),
            },
            Kind::Done,
        )
    }

    fn detach(&mut self, target: &Target) {
        let region = {
            let mut st = lock(&self.sh.state);
            st.attached.retain(|a| &a.target != target);
            st.regions.remove(target)
        };
        self.tell(Request::Detach {
            target: target.clone(),
        });
        // The emulator lets go of the region when its stream stops; the file goes a moment later, off this thread.
        if let Some(r) = region {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500)); // sync-ok: a detached worker's pause
                drop(r);
            });
        }
    }

    fn pause(&mut self, target: &Target, paused: bool) {
        for a in lock(&self.sh.state)
            .attached
            .iter_mut()
            .filter(|a| &a.target == target)
        {
            a.paused = paused;
        }
        let target = target.clone();
        self.tell(if paused {
            Request::Pause { target }
        } else {
            Request::Resume { target }
        });
    }

    fn touch(&mut self, target: &Target, touch: Touch) {
        self.tell(Request::Touch {
            target: target.clone(),
            touch,
        });
    }

    fn key(&mut self, target: &Target, key: Key) {
        self.tell(Request::Key {
            target: target.clone(),
            key,
        });
    }

    fn button(&mut self, target: &Target, button: Button) {
        self.tell(Request::Button {
            target: target.clone(),
            button,
        });
    }

    fn text(&mut self, target: &Target, text: &str) -> ReqId {
        self.request(
            Request::Text {
                target: target.clone(),
                text: text.to_string(),
            },
            Kind::Done,
        )
    }

    fn snapshot(&mut self, target: &Target) -> ReqId {
        self.request(
            Request::Snapshot {
                target: target.clone(),
            },
            Kind::Png,
        )
    }

    fn ui_tree(&mut self, target: &Target) -> ReqId {
        self.request(
            Request::UiTree {
                target: target.clone(),
                at: None,
            },
            Kind::Tree,
        )
    }

    fn launch(&mut self, target: &Target, bundle_id: &str) -> ReqId {
        self.request(
            Request::Launch {
                target: target.clone(),
                bundle_id: bundle_id.to_string(),
                args: Vec::new(),
            },
            Kind::Done,
        )
    }

    fn install(&mut self, target: &Target, app: &Path) -> ReqId {
        self.request(
            Request::Install {
                target: target.clone(),
                path: app.to_string_lossy().to_string(),
            },
            Kind::Done,
        )
    }

    fn events(&self) -> &Receiver<SourceEvent> {
        &self.rx
    }

    fn hello(&self) -> Option<Hello> {
        lock(&self.sh.state).hello.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Orientation, Size};
    use crate::source::{DeviceState, Phase, Screen};

    fn target() -> Target {
        Target::emu("Flare_Pixel").unwrap()
    }

    #[cfg(target_os = "macos")]
    /// Not a test on its own: a guard started by these tests runs as this test.
    #[test]
    fn guard_entry() {
        flare_proc::guard::run_if_guard();
    }

    fn emu_hello(host: HostKind, protocol: u32) -> Hello {
        Hello {
            protocol,
            version: "0.1.0".into(),
            host,
            coresim_version: String::new(),
            simkit_version: String::new(),
            xcode_version: String::new(),
            xcode_path: String::new(),
            input_path: None,
            ax: false,
            facts: [("emulator_version".to_string(), "37.3.10".to_string())]
                .into_iter()
                .collect(),
        }
    }

    /// Not a test on its own: run by the tests below as a fake `flare-emu-host` (`FLARE_FAKE_EMU_HOST` says how).
    #[test]
    fn fake_emu_host() {
        let Ok(mode) = std::env::var("FLARE_FAKE_EMU_HOST") else {
            return;
        };
        let dir = PathBuf::from(std::env::var("FLARE_FAKE_EMU_DIR").unwrap_or_default());
        let append = |file: &str, line: &str| {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(file))
            {
                let _ = writeln!(f, "{line}");
            }
        };
        append("starts", &std::process::id().to_string());
        let h = match mode.as_str() {
            "sim" => emu_hello(HostKind::Sim, PROTOCOL),
            "mismatch" => emu_hello(HostKind::Emu, 99),
            _ => emu_hello(HostKind::Emu, PROTOCOL),
        };
        let mut out = std::io::stdout();
        let say = |out: &mut std::io::Stdout, l: &HostLine| {
            let _ = writeln!(out, "{}", to_line(l));
            let _ = out.flush();
        };
        say(&mut out, &HostLine::Hello(h));
        if mode == "crash" {
            std::process::exit(1);
        }
        let t = target();
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            append("lines", &line);
            let Some(c) = from_line::<Control>(&line) else {
                continue;
            };
            match c.req {
                Request::Quit => break,
                Request::List => say(
                    &mut out,
                    &HostLine::Ok {
                        id: c.id,
                        result: serde_json::json!({ "devices": [crate::source::DeviceInfo {
                            target: t.clone(),
                            name: "Flare Pixel".into(),
                            kind: "Android".into(),
                            os: "Android 37".into(),
                            state: DeviceState::Booted,
                            booted_by_flare: false,
                        }] }),
                    },
                ),
                Request::Attach { target } => {
                    say(
                        &mut out,
                        &HostLine::Ok {
                            id: c.id,
                            result: Value::Null,
                        },
                    );
                    let screen =
                        Screen::from_dpi(Size::new(1080.0, 2400.0), 420.0, Orientation::Rot0)
                            .unwrap();
                    say(
                        &mut out,
                        &HostLine::Frames {
                            target,
                            device: 1,
                            ring: 0,
                            screen,
                        },
                    );
                }
                Request::Region { target, .. } => {
                    say(
                        &mut out,
                        &HostLine::Ok {
                            id: c.id,
                            result: Value::Null,
                        },
                    );
                    say(
                        &mut out,
                        &HostLine::Region {
                            target,
                            seq: 7,
                            t_us: 1_000,
                            w: 1080,
                            h: 2400,
                            rotation: Orientation::Rot0,
                        },
                    );
                }
                Request::Snapshot { .. } => say(
                    &mut out,
                    &HostLine::Ok {
                        id: c.id,
                        result: crate::wire::png_result(b"\x89PNG\r\n\x1a\nfake"),
                    },
                ),
                Request::UiTree { .. } => say(
                    &mut out,
                    &HostLine::Err {
                        id: c.id,
                        code: "unavailable".into(),
                        message: "no tree yet".into(),
                    },
                ),
                _ => say(
                    &mut out,
                    &HostLine::Ok {
                        id: c.id,
                        result: Value::Null,
                    },
                ),
            }
        }
        std::process::exit(0);
    }

    fn source(mode: &str, dir: &Path, launcher: Option<Launcher>) -> EmuSource {
        #[cfg(target_os = "macos")]
        flare_proc::guard::use_test_entry("emu_client::tests::guard_entry").unwrap();
        let mut cfg = EmuConfig::new(
            std::env::current_exe().unwrap(),
            dir.join("logs/emu-host.log"),
            // No real SDK or discovery folder: nothing here reaches a real emulator.
            Places {
                home: Some(dir.join("home")),
                local_app_data: Some(dir.join("local")),
                ..Places::default()
            },
        );
        cfg.pre_args = [
            "emu_client::tests::fake_emu_host",
            "--exact",
            "--nocapture",
            "--",
        ]
        .map(String::from)
        .to_vec();
        cfg.env = vec![
            ("FLARE_FAKE_EMU_HOST".into(), mode.into()),
            (
                "FLARE_FAKE_EMU_DIR".into(),
                dir.to_string_lossy().to_string(),
            ),
        ];
        cfg.restart_delay = Duration::from_millis(50);
        cfg.region_dir = Some(dir.join("running/flare"));
        cfg.launcher = launcher;
        EmuSource::start(cfg)
    }

    fn starts(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("starts")).map_or(0, |t| t.lines().count())
    }

    fn sent(dir: &Path) -> Vec<Control> {
        std::fs::read_to_string(dir.join("lines"))
            .unwrap_or_default()
            .lines()
            .filter_map(from_line::<Control>)
            .collect()
    }

    fn until(
        s: &EmuSource,
        limit: Duration,
        want: impl Fn(&SourceEvent) -> bool,
    ) -> Vec<SourceEvent> {
        let end = Instant::now() + limit;
        let mut seen = Vec::new();
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match s.events().recv_timeout(left) {
                Ok(e) => {
                    let hit = want(&e);
                    seen.push(e);
                    if hit {
                        return seen;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("no matching event in {limit:?}; saw {seen:?}");
    }

    fn answered(id: ReqId) -> impl Fn(&SourceEvent) -> bool {
        move |e| matches!(e, SourceEvent::Answer { id: i, .. } if *i == id)
    }

    #[test]
    fn only_an_emu_host_of_this_protocol_is_taken() {
        assert_eq!(check_hello(&emu_hello(HostKind::Emu, PROTOCOL)), Ok(()));
        assert!(
            check_hello(&emu_hello(HostKind::Sim, PROTOCOL))
                .unwrap_err()
                .contains("sim host")
        );
        assert!(
            check_hello(&emu_hello(HostKind::Emu, 2))
                .unwrap_err()
                .contains("protocol 2")
        );
    }

    #[test]
    fn the_emu_host_is_restarted_three_times_then_broken() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = source("crash", dir.path(), None);
        let seen = until(&s, Duration::from_secs(60), |e| {
            matches!(e, SourceEvent::Broken(_))
        });
        assert!(matches!(seen.last(), Some(SourceEvent::Broken(w)) if w == KEPT_STOPPING));
        assert_eq!(starts(dir.path()), 1 + MAX_RESTARTS);
        assert_eq!(s.starts() as usize, 1 + MAX_RESTARTS);
        // Broken: requests answer unavailable with the reason, at once.
        let id = s.snapshot(&target());
        let seen = until(&s, Duration::from_secs(5), answered(id));
        let Some(SourceEvent::Answer { result: Err(e), .. }) = seen.last() else {
            panic!("{seen:?}")
        };
        assert_eq!(e.fail, Fail::Unavailable);
        assert!(e.message.contains(KEPT_STOPPING), "{}", e.message);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            starts(dir.path()),
            1 + MAX_RESTARTS,
            "no start after it is broken"
        );
    }

    #[test]
    fn a_sim_host_or_another_protocol_is_broken_and_not_restarted() {
        for mode in ["sim", "mismatch"] {
            let dir = tempfile::tempdir().unwrap();
            let s = source(mode, dir.path(), None);
            let seen = until(&s, Duration::from_secs(30), |e| {
                matches!(e, SourceEvent::Broken(_))
            });
            let Some(SourceEvent::Broken(why)) = seen.last() else {
                unreachable!()
            };
            assert!(why.contains("reinstall"), "{why}");
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(
                starts(dir.path()),
                1,
                "{mode}: a broken install is not restarted"
            );
        }
    }

    /// Rule 1363: Flare makes the device's frame region when the emu host says its screen, hands over its URL, gets
    /// frame records back as `Region` events, and deletes the file on detach.
    #[test]
    fn an_attached_device_gets_a_region_from_flare_and_frames_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = source("serve", dir.path(), None);
        let t = target();
        let id = s.attach(&t);
        let seen = until(&s, Duration::from_secs(30), |e| {
            matches!(e, SourceEvent::Region { .. })
        });
        assert!(seen.iter().any(
            |e| matches!(e, SourceEvent::Answer { id: i, result: Ok(Answer::Done) } if *i == id)
        ));
        assert!(
            seen.iter()
                .any(|e| matches!(e, SourceEvent::Frames { target, ring: 1, .. } if *target == t))
        );
        let Some(SourceEvent::Region {
            target,
            seq,
            size_px,
            rotation,
            ..
        }) = seen.last()
        else {
            unreachable!()
        };
        assert_eq!(
            (target, *seq, *size_px, *rotation),
            (&t, 7, Size::new(1080.0, 2400.0), Orientation::Rot0)
        );
        let path = s.region_path(&t).expect("a region");
        assert!(
            path.starts_with(dir.path().join("running/flare")),
            "{}",
            path.display()
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            region::bytes_for(1080, 2400)
        );
        let given = sent(dir.path())
            .into_iter()
            .find_map(|c| match c.req {
                Request::Region { url, bytes, .. } => Some((url, bytes)),
                _ => None,
            })
            .expect("the region was handed over");
        assert_eq!(given.0, region::url(&path));
        assert_eq!(given.1, region::bytes_for(1080, 2400));
        // Paused and resumed: the emu host is told.
        s.pause(&t, true);
        s.pause(&t, false);
        // Detached: the file goes.
        s.detach(&t);
        let end = Instant::now() + Duration::from_secs(10);
        while path.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!path.exists(), "the region is deleted on detach");
        assert_eq!(s.region_path(&t), None);
        let ops: Vec<&'static str> = sent(dir.path())
            .iter()
            .map(|c| match c.req {
                Request::Attach { .. } => "attach",
                Request::Region { .. } => "region",
                Request::Pause { .. } => "pause",
                Request::Resume { .. } => "resume",
                Request::Detach { .. } => "detach",
                _ => "other",
            })
            .collect();
        assert_eq!(ops, ["attach", "region", "pause", "resume", "detach"]);
    }

    /// Rule 1366: Flare starts the emulator itself (detached, outside the guarded emu host), then the emu host waits for
    /// it; the list marks what Flare booted.
    #[test]
    fn a_boot_starts_the_emulator_in_flare_then_waits_in_the_emu_host() {
        let dir = tempfile::tempdir().unwrap();
        let launched = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&launched);
        let launcher: Launcher = Arc::new(move |t: &Target| {
            lock(&l).push(t.clone());
            Ok(())
        });
        let mut s = source("serve", dir.path(), Some(launcher));
        let t = target();
        let id = s.boot(&t);
        let seen = until(&s, Duration::from_secs(30), answered(id));
        assert!(
            matches!(
                seen.last(),
                Some(SourceEvent::Answer {
                    result: Ok(Answer::Done),
                    ..
                })
            ),
            "{seen:?}"
        );
        assert_eq!(*lock(&launched), std::slice::from_ref(&t));
        assert_eq!(s.booted(), std::slice::from_ref(&t));
        assert!(
            sent(dir.path())
                .iter()
                .any(|c| matches!(&c.req, Request::Boot { target } if *target == t))
        );
        let id = s.list();
        let seen = until(&s, Duration::from_secs(30), answered(id));
        let Some(SourceEvent::Answer {
            result: Ok(Answer::Devices(l)),
            ..
        }) = seen.last()
        else {
            panic!("{seen:?}")
        };
        assert!(l[0].booted_by_flare, "Flare booted it");
        // A launch that fails answers at once, and nothing is marked.
        let launcher: Launcher =
            Arc::new(|_t: &Target| Err("No virtual device named X on this computer.".into()));
        let dir2 = tempfile::tempdir().unwrap();
        let mut s2 = source("serve", dir2.path(), Some(launcher));
        let id = s2.boot(&t);
        let seen = until(&s2, Duration::from_secs(30), answered(id));
        let Some(SourceEvent::Answer { result: Err(e), .. }) = seen.last() else {
            panic!("{seen:?}")
        };
        assert!(e.message.contains("No virtual device"), "{}", e.message);
        assert!(s2.booted().is_empty());
    }

    /// Rule 1368: text goes to the emu host as text (keys or the device's clipboard there); input reaches it in order.
    #[test]
    fn input_text_and_snapshots_go_to_the_emu_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = source("serve", dir.path(), None);
        let t = target();
        s.touch(&t, Touch::one(Phase::Start, 10.0, 20.0));
        s.touch(&t, Touch::one(Phase::End, 10.0, 20.0));
        s.key(
            &t,
            Key {
                usage: 0x04,
                down: true,
            },
        );
        s.button(&t, Button::Back);
        let id = s.text(&t, "bl\u{e5}b\u{e6}r");
        until(&s, Duration::from_secs(30), answered(id));
        let id = s.snapshot(&t);
        let seen = until(&s, Duration::from_secs(30), answered(id));
        assert!(matches!(
            seen.last(),
            Some(SourceEvent::Answer {
                result: Ok(Answer::Png(_)),
                ..
            })
        ));
        let id = s.ui_tree(&t);
        let seen = until(&s, Duration::from_secs(30), answered(id));
        assert!(
            matches!(seen.last(), Some(SourceEvent::Answer { result: Err(e), .. }) if e.fail == Fail::Unavailable)
        );
        let reqs: Vec<Request> = sent(dir.path()).into_iter().map(|c| c.req).collect();
        assert!(matches!(&reqs[0], Request::Touch { touch, .. } if touch.phase == Phase::Start));
        assert!(matches!(&reqs[1], Request::Touch { touch, .. } if touch.phase == Phase::End));
        assert!(matches!(&reqs[2], Request::Key { key, .. } if key.usage == 0x04));
        assert!(matches!(
            &reqs[3],
            Request::Button {
                button: Button::Back,
                ..
            }
        ));
        assert!(matches!(&reqs[4], Request::Text { text, .. } if text == "bl\u{e5}b\u{e6}r"));
        assert!(s.hello().is_some_and(|h| h.host == HostKind::Emu));
    }
}
