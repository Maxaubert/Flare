//! Serving Flare: control lines on stdin, answers, events and frame records on stdout (`wire.rs`, a `hello` first).
//!
//! Threads: one reads stdin, one writes stdout (whole lines, nothing else ever prints there), and a small tokio
//! runtime runs every gRPC call. A request that takes time (boot, list, adb, a snapshot) runs as a task and answers
//! when done, so the control loop never waits (rule 34 in spirit: a slow emulator never holds up another request).
//!
//! Per attached emulator: the held `streamInputEvent` (touches, keys), the frame stream into Flare's region while
//! not paused (rule 1365), and a `getStatus` every minute (rule 1366: the emulator's idle timeout never fires while
//! Flare runs, paused or not). The emu host never starts an emulator: Flare does, outside this guarded process, so
//! the guard that ends this host with Flare never ends an emulator (it stops by itself after its idle timeout).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use flare_device::cap::FrameCap;
use flare_device::emu::discovery::{self, Access, Running};
use flare_device::emu::sdk::{self, Found, Sdk};
use flare_device::emu::{Places, avds, boot, flare_avd, keys};
use flare_device::geom::{Orientation, Size};
use flare_device::job::Fail;
use flare_device::source::{
    Button, DeviceInfo, DeviceState, FrameMode, InputState, Key, Phase, Screen, Touch,
};
use flare_device::target::Target;
use flare_device::wire::{
    Control, Hello, HostKind, HostLine, PROTOCOL, Request, SetupEvent, SetupStep, from_line,
    png_result, to_line,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::grpc::{self, Client};
use crate::log::Log;
use crate::pb;

/// How often the frame counts go to the log while a stream runs.
const STATS_EVERY: Duration = Duration::from_secs(60);
/// A `getStatus` that fails this many times in a row means the emulator is gone.
const STATUS_FAILS: u32 = 3;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The stdout writer: one thread, whole lines.
#[derive(Clone)]
pub struct Out(std::sync::mpsc::Sender<String>);

impl Out {
    pub fn start() -> Out {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut o = std::io::stdout().lock();
            for l in rx {
                if writeln!(o, "{l}").and_then(|()| o.flush()).is_err() {
                    break;
                }
            }
        });
        Out(tx)
    }

    pub fn line(&self, l: &HostLine) {
        let _ = self.0.send(to_line(l));
    }
}

/// One attached emulator.
struct Device {
    target: Target,
    client: Client,
    /// The console port's serial for adb (`emulator-5554`).
    serial: Option<String>,
    /// The portrait screen and pixels per dp (rule 1367).
    portrait: Size,
    scale: f64,
    orientation: Mutex<Orientation>,
    input: Mutex<Option<mpsc::UnboundedSender<pb::InputEvent>>>,
    /// Contacts down now (lifted on detach and on a new input stream).
    down: Mutex<HashSet<i32>>,
    region: Mutex<Option<(String, u64)>>,
    paused: AtomicBool,
    stream: Mutex<Option<tokio::task::JoinHandle<()>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The `frames` line's ring number.
    ring: u32,
    /// The agent helper (PR A3, rule 1369): the UI tree and set-text. None without an emulator's adb serial.
    agent: Option<Arc<crate::agent::Helper>>,
    /// The emulator's version, for the fallbacks' log lines.
    version: String,
    /// The frame mode last said to Flare, and why (PR A7).
    told: Mutex<(FrameMode, String)>,
    /// Why the frames last fell back from the shared memory (PR A7).
    fell_back: Mutex<String>,
}

impl Device {
    fn send(&self, e: pb::InputEvent) -> bool {
        lock(&self.input)
            .as_ref()
            .is_some_and(|i| i.send(e).is_ok())
    }
}

/// The emu host.
pub struct Host {
    out: Out,
    pub log: Log,
    places: Places,
    /// The SDK the search found; found again after a setup (PR A2b).
    sdk: Mutex<Found>,
    /// The hypervisor check's answer (Windows), refreshed by the `accel` setup step.
    accel: Mutex<Option<flare_device::emu::accel::Accel>>,
    /// The running setup's cancel flag (one setup at a time).
    setup: Mutex<Option<Arc<AtomicBool>>>,
    devices: Mutex<HashMap<Target, Arc<Device>>>,
    /// `getStatus` keep-alives of emulators this host booted or attached, by target (rule 1366).
    keep: Mutex<HashMap<Target, tokio::task::JoinHandle<()>>>,
    rt: tokio::runtime::Handle,
    rings: AtomicU32,
    epoch: Instant,
    restarted: bool,
}

impl Host {
    fn ok(&self, id: u64, result: Value) {
        self.out.line(&HostLine::Ok { id, result });
    }

    fn err(&self, id: u64, fail: Fail, message: impl Into<String>) {
        self.out.line(&HostLine::Err {
            id,
            code: fail.code().to_string(),
            message: message.into(),
        });
    }

    fn now_us(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn device(&self, t: &Target) -> Option<Arc<Device>> {
        lock(&self.devices).get(t).cloned()
    }

    fn discovery_dir(&self) -> Option<PathBuf> {
        self.places.discovery_dir()
    }

    /// BLOCKING (reads the discovery folder).
    fn running(&self) -> Vec<Running> {
        self.discovery_dir()
            .map(|d| discovery::read_all(&d))
            .unwrap_or_default()
    }
}

/// The emu host's facts for its `hello` (no secret: rule 1362).
pub fn hello(sdk: &Found, accel: Option<&flare_device::emu::accel::Accel>) -> Hello {
    let mut facts = BTreeMap::new();
    let (s, state) = match sdk {
        Found::Ready(s) => (Some(s), "ready"),
        Found::TooOld(s) => (Some(s), "too_old"),
        Found::Missing => (None, "missing"),
    };
    facts.insert("sdk".to_string(), state.to_string());
    facts.insert("floor".to_string(), sdk::floor().to_string());
    if let Some(s) = s {
        facts.insert("sdk_path".to_string(), s.root.to_string_lossy().to_string());
        facts.insert("sdk_own".to_string(), s.flare_own.to_string());
        facts.insert("emulator_version".to_string(), s.emulator_version.clone());
    }
    if let Some(a) = accel {
        facts.insert("accel".to_string(), a.kind().to_string());
        facts.insert("accel_usable".to_string(), a.usable().to_string());
    }
    // The agent helper's jar (PR A3): its short hash, or `missing` when none with the expected hash is here.
    facts.insert("agent_jar".to_string(), crate::agent::jar_fact());
    Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        host: HostKind::Emu,
        coresim_version: String::new(),
        simkit_version: String::new(),
        xcode_version: String::new(),
        xcode_path: String::new(),
        input_path: None,
        ax: false,
        facts,
    }
}

/// BLOCKING, at most 20 s: `emulator -accel-check`.
pub fn accel_check(sdk: &Sdk) -> Result<flare_device::emu::accel::Accel, String> {
    let mut c = crate::run::command(&sdk.emulator());
    c.arg("-accel-check");
    let r = crate::run::run(c, Duration::from_secs(20))?;
    flare_device::emu::accel::parse(&r.stdout)
        .ok_or_else(|| "the accel check said nothing readable".to_string())
}

/// Serves Flare until stdin closes or `quit`. Returns the exit code.
pub fn serve(log: Log, restarted: bool) -> i32 {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("emu-host")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log.line(&format!("no runtime: {e}"));
            return 3;
        }
    };
    let places = Places::from_env();
    let found = sdk::find(&places);
    let accel = match &found {
        Found::Ready(s) if cfg!(windows) => accel_check(s)
            .map_err(|e| log.line(&format!("accel check: {e}")))
            .ok(),
        _ => None,
    };
    let out = Out::start();
    // A setup the guard ended with Flare left its temporary folder: remove it (PR A2b).
    if let Some(s) = places.flare_sdk() {
        crate::setup::sweep(&s);
    }
    let h = hello(&found, accel.as_ref());
    log.line(&format!(
        "emu host {} (protocol {PROTOCOL}): {:?}{}",
        h.version,
        h.facts,
        if restarted { ", restarted" } else { "" }
    ));
    out.line(&HostLine::Hello(h));
    let host = Arc::new(Host {
        out,
        log,
        places,
        sdk: Mutex::new(found),
        accel: Mutex::new(accel),
        setup: Mutex::new(None),
        devices: Mutex::new(HashMap::new()),
        keep: Mutex::new(HashMap::new()),
        rt: rt.handle().clone(),
        rings: AtomicU32::new(1),
        epoch: Instant::now(),
        restarted,
    });
    let stdin = std::io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Some(c) = from_line::<Control>(&line) else {
            if !line.trim().is_empty() {
                host.log.line(&format!(
                    "a line that is not a request: {:?}",
                    line.chars().take(120).collect::<String>()
                ));
            }
            continue;
        };
        if !handle(&host, c) {
            break;
        }
    }
    // Flare went: let every finger go, then stop.
    let all: Vec<Arc<Device>> = lock(&host.devices).drain().map(|(_, d)| d).collect();
    for d in all {
        detach_device(&d);
    }
    host.log.line("emu host stops");
    rt.shutdown_timeout(Duration::from_millis(500));
    0
}

/// One control line. False to end.
fn handle(host: &Arc<Host>, c: Control) -> bool {
    let id = c.id;
    match c.req {
        Request::Hello { protocol } => {
            if protocol != PROTOCOL {
                host.err(
                    id,
                    Fail::Unavailable,
                    format!("protocol {protocol}, this emu host speaks {PROTOCOL}"),
                );
                return false;
            }
            host.ok(id, Value::Null);
        }
        Request::Quit => {
            host.ok(id, Value::Null);
            return false;
        }
        Request::List => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                let hh = Arc::clone(&h);
                let r = tokio::task::spawn_blocking(move || list(&hh)).await;
                match r {
                    Ok(l) => h.ok(id, json!({ "devices": l })),
                    Err(e) => h.err(id, Fail::Unavailable, e.to_string()),
                }
            });
        }
        Request::Boot { target } => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                match boot_wait(&h, &target).await {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => h.err(id, f, m),
                }
            });
        }
        Request::Shutdown { target } => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                match shutdown(&h, &target).await {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => h.err(id, f, m),
                }
            });
        }
        Request::Attach { target } => {
            if host.device(&target).is_some() {
                host.ok(id, Value::Null);
                return true;
            }
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                match attach(&h, &target).await {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => {
                        h.log.line(&format!("{target}: attach failed: {m}"));
                        h.err(id, f, m);
                    }
                }
            });
        }
        Request::Region { target, url, bytes } => match host.device(&target) {
            None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            Some(d) => {
                let need =
                    flare_device::emu::region::bytes_for(d.portrait.w as u32, d.portrait.h as u32);
                if bytes < need {
                    host.err(
                        id,
                        Fail::Unavailable,
                        format!("the region holds {bytes} bytes, the screen needs {need}"),
                    );
                } else {
                    *lock(&d.region) = Some((url, bytes));
                    restart_stream(host, &d);
                    host.ok(id, Value::Null);
                }
            }
        },
        Request::Detach { target } => {
            let d = lock(&host.devices).remove(&target);
            if let Some(d) = d {
                detach_device(&d);
                host.log.line(&format!("{target}: detached"));
            }
            host.ok(id, Value::Null);
        }
        Request::Pause { target } | Request::Resume { target }
            if host.device(&target).is_none() =>
        {
            host.err(id, Fail::Unavailable, format!("{target} is not attached"));
        }
        Request::Pause { target } => {
            if let Some(d) = host.device(&target) {
                d.paused.store(true, Ordering::Relaxed);
                if let Some(s) = lock(&d.stream).take() {
                    s.abort();
                }
                host.log.line(&format!("{target}: paused"));
            }
            host.ok(id, Value::Null);
        }
        Request::Resume { target } => {
            if let Some(d) = host.device(&target) {
                d.paused.store(false, Ordering::Relaxed);
                restart_stream(host, &d);
                host.log.line(&format!("{target}: resumed"));
            }
            host.ok(id, Value::Null);
        }
        Request::Touch { target, touch } => {
            if let Some(d) = host.device(&target) {
                send_touch(&d, touch);
            }
        }
        Request::Key { target, key } => {
            if let Some(d) = host.device(&target) {
                d.send(key_event(key));
            }
        }
        Request::Button { target, button } => match host.device(&target) {
            None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            Some(d) => {
                let h = Arc::clone(host);
                host.rt.spawn(async move {
                    match press(&d, button).await {
                        Ok(()) => h.ok(id, Value::Null),
                        Err((f, m)) => h.err(id, f, m),
                    }
                });
            }
        },
        Request::Text { target, text } => match host.device(&target) {
            None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            Some(d) => {
                let h = Arc::clone(host);
                host.rt.spawn(async move {
                    match type_text(&d, &text).await {
                        Ok(()) => h.ok(id, Value::Null),
                        Err((f, m)) => h.err(id, f, m),
                    }
                });
            }
        },
        Request::Snapshot { target } => match host.device(&target) {
            None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            Some(d) => {
                let h = Arc::clone(host);
                host.rt.spawn(async move {
                    let started = Instant::now();
                    match snapshot(&h, &d).await {
                        Ok(png) => {
                            h.log.line(&format!(
                                "{}: snapshot {} KB in {} ms",
                                d.target,
                                png.len() / 1024,
                                started.elapsed().as_millis()
                            ));
                            h.ok(id, png_result(&png));
                        }
                        Err((f, m)) => h.err(id, f, m),
                    }
                });
            }
        },
        Request::UiTree { target, .. } => match host.device(&target) {
            None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            Some(d) => match d.agent.clone() {
                None => host.err(
                    id,
                    Fail::Unavailable,
                    format!("{target}: no adb serial, so no UI tree"),
                ),
                Some(a) => {
                    let h = Arc::clone(host);
                    let (scale, screen) = (d.scale, screen_dp(&d));
                    host.rt.spawn(async move {
                        let started = Instant::now();
                        let r = tokio::task::spawn_blocking(move || a.tree_settled(scale, screen))
                            .await
                            .unwrap_or_else(|e| Err((Fail::Unavailable, e.to_string())));
                        match r {
                            Ok(v) => {
                                h.log.line(&format!(
                                    "{target}: tree ({}) in {} ms",
                                    v["path"].as_str().unwrap_or("?"),
                                    started.elapsed().as_millis()
                                ));
                                h.ok(id, v);
                            }
                            Err((f, m)) => h.err(id, f, m),
                        }
                    });
                }
            },
        },
        Request::Launch {
            target,
            bundle_id,
            args,
        } => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                let hh = Arc::clone(&h);
                let r = tokio::task::spawn_blocking(move || {
                    let (adb, serial) = adb_for(&hh, &target)?;
                    crate::adb::launch(&adb, &serial, &bundle_id, &args)
                })
                .await
                .unwrap_or_else(|e| Err((Fail::Unavailable, e.to_string())));
                match r {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => h.err(id, f, m),
                }
            });
        }
        Request::Setup { step } => setup_step(host, id, step),
        // Rule 1366 (PR A7): an emulator Flare booted stays alive while Flare runs, attached or not. One that no
        // longer runs is said to be shut down, so Flare forgets it.
        Request::Keep { target } => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                if lock(&h.keep).contains_key(&target) {
                    h.ok(id, Value::Null);
                    return;
                }
                match reach(&h, &target).await {
                    Ok((_, client)) => {
                        keep_alive(&h, &target, client);
                        h.log
                            .line(&format!("{target}: kept alive while Flare runs"));
                        h.ok(id, Value::Null);
                    }
                    Err((f, m)) => {
                        let not_running = m == format!("{target} is not running");
                        h.err(id, f, m);
                        if not_running {
                            gone(&h, &target);
                        }
                    }
                }
            });
        }
        Request::Install { target, path } => {
            let h = Arc::clone(host);
            host.rt.spawn(async move {
                let hh = Arc::clone(&h);
                let r = tokio::task::spawn_blocking(move || {
                    let (adb, serial) = adb_for(&hh, &target)?;
                    crate::adb::install(&adb, &serial, std::path::Path::new(&path))
                })
                .await
                .unwrap_or_else(|e| Err((Fail::Unavailable, e.to_string())));
                match r {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => h.err(id, f, m),
                }
            });
        }
    }
    true
}

type Failed = (Fail, String);

fn ready_sdk(host: &Host) -> Result<Sdk, Failed> {
    match &*lock(&host.sdk) {
        Found::Ready(s) => Ok(s.clone()),
        Found::TooOld(s) => Err((
            Fail::Unavailable,
            format!(
                "the Android emulator {} is older than {} (update it)",
                s.emulator_version,
                sdk::floor()
            ),
        )),
        Found::Missing => Err((
            Fail::Unavailable,
            "no Android emulator on this computer".to_string(),
        )),
    }
}

/// BLOCKING: adb and the device's serial.
fn adb_for(host: &Host, target: &Target) -> Result<(PathBuf, String), Failed> {
    let s = ready_sdk(host)?;
    let serial = host
        .device(target)
        .and_then(|d| d.serial.clone())
        .or_else(|| discovery::of_avd(&host.running(), target.id()).and_then(Running::serial))
        .ok_or((Fail::Unavailable, format!("{target} is not running")))?;
    Ok((s.adb(), serial))
}

/// BLOCKING: every AVD, with its state from the discovery folder.
fn list(host: &Host) -> Vec<DeviceInfo> {
    let running = host.running();
    avds::list(&avds::homes(&host.places))
        .into_iter()
        .filter_map(|a| {
            let target = a.target()?;
            let state = if discovery::of_avd(&running, &a.name).is_some() {
                DeviceState::Booted
            } else {
                DeviceState::Shutdown
            };
            Some(DeviceInfo {
                target,
                os: a.os(),
                name: a.display_name,
                kind: "Android".to_string(),
                state,
                booted_by_flare: false,
            })
        })
        .collect()
}

/// One step of Android setup (PR A2b, rule 1361). Every step that reads the network or the disk runs on a blocking
/// worker; the control loop only starts it.
fn setup_step(host: &Arc<Host>, id: u64, step: SetupStep) {
    use crate::setup::Stop;
    let busy = lock(&host.setup).is_some();
    match step {
        SetupStep::Cancel => {
            if let Some(c) = lock(&host.setup).as_ref() {
                c.store(true, Ordering::Relaxed);
                host.log.line("setup: cancel");
            }
            host.ok(id, Value::Null);
        }
        SetupStep::Plan | SetupStep::Start { .. } if busy => {
            host.err(id, Fail::Busy, "Android setup is running");
        }
        SetupStep::Plan => {
            let h = Arc::clone(host);
            host.rt
                .spawn_blocking(move || match plan_now(&h, &AtomicBool::new(false)) {
                    Ok((plan, sdk_root, free)) => h.ok(id, plan_json(&plan, &sdk_root, free)),
                    Err(e) => {
                        h.log.line(&format!("setup: plan failed: {e:?}"));
                        h.err(id, Fail::Unavailable, stop_line(e));
                    }
                });
        }
        SetupStep::Start { accepted } => {
            let cancel = Arc::new(AtomicBool::new(false));
            *lock(&host.setup) = Some(Arc::clone(&cancel));
            let h = Arc::clone(host);
            host.rt.spawn_blocking(move || {
                let r = start(&h, &accepted, &cancel);
                *lock(&h.setup) = None;
                match r {
                    Ok(done) => {
                        // Flare's folder changed: find the SDK again, and check the hypervisor with it.
                        let found = sdk::find(&h.places);
                        let accel = match &found {
                            Found::Ready(sd) if cfg!(windows) => accel_check(sd).ok(),
                            _ => None,
                        };
                        let facts = hello(&found, accel.as_ref()).facts;
                        *lock(&h.sdk) = found;
                        *lock(&h.accel) = accel;
                        h.log.line(&format!("setup: done: {done:?}; {facts:?}"));
                        h.ok(
                            id,
                            json!({ "installed": done.installed, "device": done.avd, "facts": facts }),
                        );
                    }
                    Err(Stop::Cancelled) => {
                        h.log.line("setup: cancelled");
                        h.err(id, Fail::Interrupted, "Android setup was cancelled");
                    }
                    Err(e) => {
                        h.log.line(&format!("setup: failed: {e:?}"));
                        h.err(id, Fail::Unavailable, stop_line(e));
                    }
                }
            });
        }
        SetupStep::Accel => {
            let h = Arc::clone(host);
            host.rt.spawn_blocking(move || {
                let r = ready_sdk(&h)
                    .and_then(|sd| accel_check(&sd).map_err(|e| (Fail::Unavailable, e)));
                match r {
                    Ok(a) => {
                        let v = json!({ "accel": a.kind(), "usable": a.usable(), "line": a.line });
                        *lock(&h.accel) = Some(a);
                        h.ok(id, v);
                    }
                    Err((f, m)) => h.err(id, f, m),
                }
            });
        }
        SetupStep::TurnOnHypervisor => {
            // Only the person's click on [Turn on] sends this (Flare's side); Windows asks through its own prompt.
            host.log
                .line("setup: turning on Windows Hypervisor Platform (dism, elevated)");
            let h = Arc::clone(host);
            std::thread::spawn(move || match crate::hyperv::turn_on() {
                Ok(restart) => {
                    h.log
                        .line(&format!("setup: hypervisor turned on, restart {restart}"));
                    h.ok(id, json!({ "restart": restart }));
                }
                Err(e) => {
                    h.log.line(&format!("setup: hypervisor: {e}"));
                    h.err(id, Fail::Unavailable, e);
                }
            });
        }
    }
}

fn stop_line(e: crate::setup::Stop) -> String {
    match e {
        crate::setup::Stop::Cancelled => "Android setup was cancelled".to_string(),
        crate::setup::Stop::Failed(m) => m,
    }
}

/// BLOCKING (the network): what Flare's own folder needs now, its folder, and the free space there.
fn plan_now(
    host: &Host,
    cancel: &AtomicBool,
) -> Result<(flare_device::emu::setup::Plan, PathBuf, Option<u64>), crate::setup::Stop> {
    use crate::setup as s;
    let cpu = flare_device::emu::setup::HostCpu::this().ok_or_else(|| {
        s::Stop::Failed("Android devices do not run on this computer".to_string())
    })?;
    let (sdk_root, avd_home) = s::folders(&host.places)?;
    let (repo, images) = s::read_lists(&s::Https::new(), cpu, cancel)?;
    let plan = flare_device::emu::setup::plan(
        &repo,
        &images,
        cpu,
        &s::have(&sdk_root, &avd_home),
        sdk::floor(),
    )?;
    let free = s::free_bytes(&sdk_root);
    Ok((plan, sdk_root, free))
}

/// The plan's answer: what is fetched, its size, the licences to show (with the hash Flare sends back on accept).
fn plan_json(
    plan: &flare_device::emu::setup::Plan,
    sdk_root: &std::path::Path,
    free: Option<u64>,
) -> Value {
    let mb = |b: u64| b / 1_000_000;
    json!({
        "needed": !plan.is_empty(),
        "update": plan.update,
        "newest_emulator": plan.newest_emulator,
        "packages": plan.get.iter().map(|g| json!({
            "path": g.remote.path,
            "version": g.remote.version,
            "mb": mb(g.archive.size),
        })).collect::<Vec<_>>(),
        "licences": plan.licences.iter().map(|(lid, text)| json!({
            "id": lid,
            "text": text,
            "hash": crate::setup::licence_hash(text),
        })).collect::<Vec<_>>(),
        "download_mb": mb(plan.download_bytes()),
        "disk_mb": mb(plan.disk_bytes()),
        "free_mb": free.map(mb),
        "enough_space": free.is_none_or(|f| f >= plan.free_bytes_needed()),
        "sdk_path": sdk_root.to_string_lossy(),
    })
}

/// BLOCKING: the setup itself, from a fresh plan (a licence that changed since the person read it is refused).
fn start(
    host: &Arc<Host>,
    accepted: &[String],
    cancel: &AtomicBool,
) -> Result<crate::setup::Done, crate::setup::Stop> {
    use crate::setup as s;
    let (plan, sdk_root, free) = plan_now(host, cancel)?;
    if plan.is_empty() {
        return Ok(s::Done::default());
    }
    // Flare never updates while a device runs (spec "Setup", Updates).
    let replaces = plan
        .get
        .iter()
        .any(|g| sdk_root.join(g.remote.dest()).exists());
    if replaces && (!lock(&host.devices).is_empty() || !host.running().is_empty()) {
        return Err(s::Stop::Failed(
            "Shut down the Android devices before the update.".to_string(),
        ));
    }
    let (_, avd_home) = s::folders(&host.places)?;
    host.log.line(&format!(
        "setup: start: {:?}, {} MB",
        plan.get.iter().map(|g| &g.remote.path).collect::<Vec<_>>(),
        plan.download_bytes() / 1_000_000
    ));
    let out = host.out.clone();
    s::install(
        &s::Https::new(),
        &plan,
        &sdk_root,
        &avd_home,
        accepted,
        free,
        cancel,
        &mut |event| out.line(&HostLine::Setup { event }),
    )
}

/// After a first boot of Flare's own device: `pm bg-dexopt-job` once, with a "Preparing" event, then the mark.
async fn prepare_first_boot(host: &Arc<Host>, target: &Target, serial: Option<String>) {
    let h = Arc::clone(host);
    let name = target.id().to_string();
    let avd = tokio::task::spawn_blocking(move || {
        avds::list(&avds::homes(&h.places))
            .into_iter()
            .find(|a| a.name == name)
    })
    .await
    .ok()
    .flatten();
    let (Some(avd), Some(serial), Ok(sd)) = (avd, serial, ready_sdk(host)) else {
        return;
    };
    if !flare_avd::needs_preparing(&avd) {
        return;
    }
    host.out.line(&HostLine::Setup {
        event: SetupEvent::Preparing {
            target: target.clone(),
        },
    });
    host.log.line(&format!("{target}: preparing (first boot)"));
    let started = Instant::now();
    let r = tokio::task::spawn_blocking(move || {
        crate::adb::prepare(&sd.adb(), &serial)?;
        std::fs::write(avd.dir.join(flare_avd::PREPARED), b"").map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    host.log.line(&format!(
        "{target}: prepared in {} s{}",
        started.elapsed().as_secs(),
        r.err()
            .map(|e| format!(" (failed: {e})"))
            .unwrap_or_default()
    ));
}

/// The running emulator of `target` that Flare may drive, with its client.
async fn reach(host: &Arc<Host>, target: &Target) -> Result<(Running, Client), Failed> {
    let h = Arc::clone(host);
    let running = tokio::task::spawn_blocking(move || h.running())
        .await
        .unwrap_or_default();
    let r = discovery::of_avd(&running, target.id())
        .cloned()
        .ok_or((Fail::Unavailable, format!("{target} is not running")))?;
    let Access::Token { port, token } = r.access() else {
        return Err((Fail::Unavailable, discovery::NO_ACCESS.to_string()));
    };
    let c = grpc::connect(port, &token)
        .await
        .map_err(|e| (Fail::Unavailable, e))?;
    Ok((r, c))
}

/// Waits for the emulator Flare started for `target`: its discovery file, then `booted` (up to
/// [`boot::BOOT_WAIT_S`]). Flare starts the process itself (see the module doc).
async fn boot_wait(host: &Arc<Host>, target: &Target) -> Result<(), Failed> {
    ready_sdk(host)?;
    let started = Instant::now();
    let limit = Duration::from_secs(boot::BOOT_WAIT_S);
    host.out.line(&HostLine::State {
        target: target.clone(),
        state: DeviceState::Booting,
    });
    host.log.line(&format!("{target}: waiting for boot"));
    let (running, client) = loop {
        match reach(host, target).await {
            Ok(rc) => break rc,
            Err((_, m)) if m == discovery::NO_ACCESS => return Err((Fail::Unavailable, m)),
            Err(_) if started.elapsed() < limit => {
                tokio::time::sleep(Duration::from_millis(500)).await
            }
            Err(_) => return Err((Fail::Busy, "Still starting...".to_string())),
        }
    };
    loop {
        let booted = client
            .clone()
            .get_status(())
            .await
            .map(|s| s.into_inner().booted)
            .unwrap_or(false);
        if booted {
            break;
        }
        if started.elapsed() >= limit {
            return Err((Fail::Busy, "Still starting...".to_string()));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    host.log.line(&format!(
        "{target}: booted in {} s",
        started.elapsed().as_secs()
    ));
    keep_alive(host, target, client);
    // Flare's own new device compiles its apps once before it is called ready (PR A2b).
    prepare_first_boot(host, target, running.serial()).await;
    host.out.line(&HostLine::State {
        target: target.clone(),
        state: DeviceState::Booted,
    });
    Ok(())
}

/// `getStatus` every minute while this host runs (rule 1366), paused or not; three failures in a row mean the
/// emulator is gone.
fn keep_alive(host: &Arc<Host>, target: &Target, client: Client) {
    let h = Arc::clone(host);
    let t = target.clone();
    let task = host.rt.spawn(async move {
        let mut fails = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(boot::KEEP_ALIVE_S)).await;
            match client.clone().get_status(()).await {
                Ok(_) => fails = 0,
                Err(e) => {
                    fails += 1;
                    h.log
                        .line(&format!("{t}: getStatus failed ({fails}): {}", e.code()));
                    if fails >= STATUS_FAILS {
                        gone(&h, &t);
                        return;
                    }
                }
            }
        }
    });
    if let Some(old) = lock(&host.keep).insert(target.clone(), task) {
        old.abort();
    }
}

/// The emulator of `target` is gone (shut down outside Flare, or it died): forget it and say so.
fn gone(host: &Arc<Host>, target: &Target) {
    let d = lock(&host.devices).remove(target);
    if let Some(d) = d {
        detach_device(&d);
    }
    if let Some(k) = lock(&host.keep).remove(target) {
        k.abort();
    }
    host.log.line(&format!("{target}: gone"));
    host.out.line(&HostLine::State {
        target: target.clone(),
        state: DeviceState::Shutdown,
    });
}

/// `setVmState(SHUTDOWN)`: a clean shutdown. Only for an emulator Flare booted (Flare's rule, the iOS spec's
/// `booted.json`); the emu host does what it is asked.
async fn shutdown(host: &Arc<Host>, target: &Target) -> Result<(), Failed> {
    let (r, c) = reach(host, target).await?;
    host.log
        .line(&format!("{target}: shut down (pid {})", r.pid));
    host.out.line(&HostLine::State {
        target: target.clone(),
        state: DeviceState::ShuttingDown,
    });
    c.clone()
        .set_vm_state(pb::VmRunState {
            state: pb::vm_run_state::RunState::Shutdown as i32,
        })
        .await
        .map_err(|e| {
            (
                Fail::Unavailable,
                format!("the emulator refused to shut down: {}", e.message()),
            )
        })?;
    // It saves its Quick Boot state and ends within about 20 s; its discovery file goes last.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(SHUTDOWN_WAIT_S) {
        let h = Arc::clone(host);
        let still = tokio::task::spawn_blocking(move || h.running())
            .await
            .unwrap_or_default()
            .iter()
            .any(|x| x.pid == r.pid);
        if !still {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    host.log.line(&format!(
        "{target}: shut down in {} s",
        started.elapsed().as_secs()
    ));
    gone(host, target);
    Ok(())
}

/// How long a shutdown waits for the emulator to end.
const SHUTDOWN_WAIT_S: u64 = 60;

/// The screen of an AVD: its `config.ini`, else what the emulator says.
async fn screen_of(running: &Running, client: &Client) -> Result<(Size, f64), Failed> {
    let dir = running.avd_dir.clone();
    let cfg = tokio::task::spawn_blocking(move || {
        let dir = dir?;
        let text = std::fs::read_to_string(dir.join("config.ini")).ok()?;
        let a = avds::parse_config("x", &dir, &dir, &text, false);
        Some((a.size_px?, a.dpi?))
    })
    .await
    .ok()
    .flatten();
    if let Some(s) = cfg {
        return Ok(s);
    }
    let d = client
        .clone()
        .get_display_configurations(())
        .await
        .map_err(|e| {
            (
                Fail::Unavailable,
                format!("no screen size: {}", e.message()),
            )
        })?
        .into_inner();
    let main = d
        .displays
        .iter()
        .find(|x| x.display == 0)
        .ok_or((Fail::Unavailable, "no main display".to_string()))?;
    let (w, h) = (
        f64::from(main.width.min(main.height)),
        f64::from(main.width.max(main.height)),
    );
    Ok((Size::new(w, h), f64::from(main.dpi)))
}

/// Attaches: client, screen, held input stream, keep-alive; then the `frames` line (Flare answers with the region).
async fn attach(host: &Arc<Host>, target: &Target) -> Result<(), Failed> {
    ready_sdk(host)?;
    let (r, client) = reach(host, target).await?;
    let (portrait, dpi) = screen_of(&r, &client).await?;
    let screen = Screen::from_dpi(portrait, dpi, Orientation::Rot0).ok_or((
        Fail::Unavailable,
        format!("a screen that does not read: {portrait:?} at {dpi} dpi"),
    ))?;
    let adb = ready_sdk(host)?.adb();
    let agent = r
        .serial()
        .and_then(|s| crate::agent::Helper::new(adb, s, host.log.clone()))
        .map(Arc::new);
    let d = Arc::new(Device {
        target: target.clone(),
        client: client.clone(),
        serial: r.serial(),
        portrait,
        scale: screen.scale,
        orientation: Mutex::new(Orientation::Rot0),
        input: Mutex::new(None),
        down: Mutex::new(HashSet::new()),
        region: Mutex::new(None),
        paused: AtomicBool::new(false),
        stream: Mutex::new(None),
        tasks: Mutex::new(Vec::new()),
        ring: host.rings.fetch_add(1, Ordering::Relaxed),
        agent,
        version: r.version.clone().unwrap_or_else(|| "?".to_string()),
        told: Mutex::new((FrameMode::Live, String::new())),
        fell_back: Mutex::new(String::new()),
    });
    open_input(host, &d);
    lock(&host.devices).insert(target.clone(), Arc::clone(&d));
    keep_alive(host, target, client);
    host.log.line(&format!(
        "{target}: attached (pid {}, emulator {}, {}x{} px at {dpi} dpi{})",
        r.pid,
        r.version.as_deref().unwrap_or("?"),
        portrait.w,
        portrait.h,
        if host.restarted {
            ", after a restart"
        } else {
            ""
        }
    ));
    host.out.line(&HostLine::Frames {
        target: target.clone(),
        device: d.ring,
        ring: 0,
        screen,
    });
    host.out.line(&HostLine::Input {
        target: target.clone(),
        state: InputState::Live,
    });
    // The agent helper starts now, off the control loop, so the first look is quick (PR A3).
    if let Some(a) = d.agent.clone() {
        host.rt.spawn_blocking(move || a.warm());
    }
    watch_changes(host, &d);
    Ok(())
}

/// While the frame stream is paused (rule 1365) no frame says the screen changed, so the agent helper's window
/// events do (PR A7, spec "Frames": the "changed" counter comes from the helper while paused): each new count becomes
/// one `changed` line, and the agent's input calls answer `changed: true`. Ends when the device is detached.
fn watch_changes(host: &Arc<Host>, d: &Arc<Device>) {
    let Some(a) = d.agent.clone() else {
        return;
    };
    let h = Arc::clone(host);
    let dd = Arc::clone(d);
    let task = host.rt.spawn(async move {
        let mut last = a.changed();
        loop {
            tokio::time::sleep(CHANGES_EVERY).await;
            if !h.device(&dd.target).is_some_and(|x| Arc::ptr_eq(&x, &dd)) {
                return;
            }
            let now = a.changed();
            if now != last {
                last = now;
                if dd.paused.load(Ordering::Relaxed) {
                    h.out.line(&HostLine::Changed {
                        target: dd.target.clone(),
                        count: now,
                    });
                }
            }
        }
    });
    lock(&d.tasks).push(task);
}

/// How often the agent helper's window count is looked at while a device is attached.
const CHANGES_EVERY: Duration = Duration::from_millis(100);

/// The screen in dp as it is shown now (the current orientation), for the tree's root.
fn screen_dp(d: &Device) -> Size {
    let shown = lock(&d.orientation).size(d.portrait);
    Size::new(shown.w / d.scale, shown.h / d.scale)
}

/// The held `streamInputEvent`: one stream for every touch and key, opened again if it drops.
fn open_input(host: &Arc<Host>, d: &Arc<Device>) {
    let (tx, rx) = mpsc::unbounded_channel::<pb::InputEvent>();
    *lock(&d.input) = Some(tx);
    let h = Arc::clone(host);
    let dd = Arc::clone(d);
    let task = host.rt.spawn(async move {
        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        let r = dd.client.clone().stream_input_event(stream).await;
        if h.device(&dd.target).is_some_and(|x| Arc::ptr_eq(&x, &dd)) {
            h.log.line(&format!(
                "{}: the input stream ended ({}), opened again",
                dd.target,
                r.err()
                    .map_or_else(|| "closed".to_string(), |e| e.code().to_string())
            ));
            h.out.line(&HostLine::Input {
                target: dd.target.clone(),
                state: InputState::Lost,
            });
            lock(&dd.down).clear();
            tokio::time::sleep(Duration::from_millis(500)).await;
            if h.device(&dd.target).is_some_and(|x| Arc::ptr_eq(&x, &dd)) {
                open_input(&h, &dd);
                h.out.line(&HostLine::Input {
                    target: dd.target.clone(),
                    state: InputState::Live,
                });
            }
        }
    });
    lock(&d.tasks).push(task);
}

/// Lets every finger go and stops the frame stream of a device no longer attached. The keep-alive stays (rule 1366:
/// it runs while Flare runs). The input sender goes last, after the lifts are queued: its stream sends them, then
/// ends by itself, and is not opened again since the device is no longer listed.
fn detach_device(d: &Arc<Device>) {
    lift_all(d);
    if let Some(s) = lock(&d.stream).take() {
        s.abort();
    }
    *lock(&d.input) = None;
    lock(&d.tasks).clear();
    // The helper and its forward end on a thread of their own (adb calls block).
    if let Some(a) = d.agent.clone() {
        std::thread::spawn(move || a.stop());
    }
}

fn touch_event(touches: Vec<pb::Touch>) -> pb::InputEvent {
    pb::InputEvent {
        r#type: Some(pb::input_event::Type::TouchEvent(pb::TouchEvent {
            touches,
            display: 0,
        })),
    }
}

/// The pressure of a finger down (pressure 0 lifts it).
const PRESSURE: i32 = 1;

fn contact(d: &Device, id: i32, x: f64, y: f64, down: bool) -> pb::Touch {
    let o = *lock(&d.orientation);
    let (px, py) = keys::touch_px(x, y, d.scale, d.portrait, o);
    pb::Touch {
        x: px,
        y: py,
        identifier: id,
        pressure: if down { PRESSURE } else { 0 },
        ..Default::default()
    }
}

/// A touch in dp (rule 1367): contact 0, and contact 1 for a pinch.
fn send_touch(d: &Device, t: Touch) {
    let down = t.phase != Phase::End;
    let mut touches = vec![contact(d, 0, t.x, t.y, down)];
    if let Some((x, y)) = t.second {
        touches.push(contact(d, 1, x, y, down));
    }
    {
        let mut open = lock(&d.down);
        for c in &touches {
            if down {
                open.insert(c.identifier);
            } else {
                open.remove(&c.identifier);
            }
        }
        // A pinch that ends with one finger still down lifts the other too.
        if !down {
            for id in open.drain() {
                touches.push(pb::Touch {
                    identifier: id,
                    pressure: 0,
                    ..Default::default()
                });
            }
        }
    }
    d.send(touch_event(touches));
}

/// Lifts every open contact (detach, focus loss, a new input stream).
fn lift_all(d: &Device) {
    let ids: Vec<i32> = lock(&d.down).drain().collect();
    if !ids.is_empty() {
        d.send(touch_event(
            ids.into_iter()
                .map(|identifier| pb::Touch {
                    identifier,
                    pressure: 0,
                    ..Default::default()
                })
                .collect(),
        ));
    }
}

fn key_event(k: Key) -> pb::InputEvent {
    use pb::keyboard_event::{KeyCodeType, KeyEventType};
    pb::InputEvent {
        r#type: Some(pb::input_event::Type::KeyEvent(pb::KeyboardEvent {
            code_type: KeyCodeType::Usb as i32,
            event_type: if k.down {
                KeyEventType::Keydown
            } else {
                KeyEventType::Keyup
            } as i32,
            key_code: keys::usb_code(k.usage),
            ..Default::default()
        })),
    }
}

async fn press(d: &Device, b: Button) -> Result<(), Failed> {
    use pb::keyboard_event::{KeyCodeType, KeyEventType};
    let Some(p) = keys::press(b) else {
        return Err((
            Fail::Unavailable,
            format!("an Android device has no {} button", b.name()),
        ));
    };
    let key = |k: pb::KeyboardEvent| pb::InputEvent {
        r#type: Some(pb::input_event::Type::KeyEvent(k)),
    };
    match p {
        keys::Press::Named(name) => {
            d.send(key(pb::KeyboardEvent {
                event_type: KeyEventType::Keypress as i32,
                key: name.to_string(),
                ..Default::default()
            }));
        }
        keys::Press::Evdev(code) => {
            d.send(key(pb::KeyboardEvent {
                code_type: KeyCodeType::Evdev as i32,
                event_type: KeyEventType::Keypress as i32,
                key_code: code,
                ..Default::default()
            }));
        }
        keys::Press::Turn(delta) => {
            let to = keys::turned(*lock(&d.orientation), delta);
            let [x, y, z] = keys::rotation_degrees(to);
            d.client
                .clone()
                .set_physical_model(pb::PhysicalModelValue {
                    target: pb::physical_model_value::PhysicalType::Rotation as i32,
                    value: Some(pb::ParameterValue {
                        data: vec![x, y, z],
                    }),
                    ..Default::default()
                })
                .await
                .map_err(|e| {
                    (
                        Fail::Unavailable,
                        format!("the emulator did not turn: {}", e.message()),
                    )
                })?;
        }
    }
    Ok(())
}

/// Types `text` (rule 1368): HID keys; text the keymap cannot type through the agent helper's set-text at the cursor
/// (PR A3), else the device's own clipboard and Ctrl+V. Never the computer's clipboard.
async fn type_text(d: &Device, text: &str) -> Result<(), Failed> {
    let steps = keys::text_steps(text);
    if let (Some(keys::TextStep::DeviceClipboard(t)), Some(a)) = (steps.first(), d.agent.clone()) {
        let t = t.clone();
        let typed = tokio::task::spawn_blocking(move || a.insert_text(&t))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if typed.is_ok() {
            return Ok(());
        }
    }
    for step in steps {
        match step {
            keys::TextStep::Keys(ks) => {
                for k in ks {
                    d.send(key_event(k));
                }
            }
            keys::TextStep::DeviceClipboard(t) => {
                d.client
                    .clone()
                    .set_clipboard(pb::ClipData { text: t })
                    .await
                    .map_err(|e| {
                        (
                            Fail::Unavailable,
                            format!("the device's clipboard: {}", e.message()),
                        )
                    })?;
            }
        }
    }
    Ok(())
}

/// A PNG at dp size of the screen as it is now (rule 1367): the emulator scales it. The reply says the screen's
/// rotation; while the stream is paused that is the only news of a turn, so a new one is kept (and said, so the panel's
/// touch map follows) and the picture is asked for again at the turned size.
async fn snapshot(host: &Host, d: &Device) -> Result<Vec<u8>, Failed> {
    for _ in 0..2 {
        let o = *lock(&d.orientation);
        let shown = o.size(d.portrait);
        let img = d
            .client
            .clone()
            .get_screenshot(pb::ImageFormat {
                format: pb::image_format::ImgFormat::Png as i32,
                width: (shown.w / d.scale).round() as u32,
                height: (shown.h / d.scale).round() as u32,
                display: 0,
                ..Default::default()
            })
            .await
            .map_err(|e| (Fail::Unavailable, format!("no screenshot: {}", e.message())))?
            .into_inner();
        if !img.image.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err((Fail::Unavailable, "the emulator sent no PNG".to_string()));
        }
        let now = keys::orientation_of(
            img.format
                .and_then(|f| f.rotation)
                .map_or(0, |r| r.rotation),
        );
        if now == o {
            return Ok(img.image);
        }
        *lock(&d.orientation) = now;
        host.out.line(&HostLine::Orientation {
            target: d.target.clone(),
            orientation: now,
        });
    }
    Err((
        Fail::Retry,
        "the device turned while it was looked at".to_string(),
    ))
}
/// (Re)starts the frame stream when the device has a region and is not paused (rule 1365). It starts on the normal
/// path (MMAP into the region) and falls back in the spec's order ("Frames", fallbacks; PR A7): when MMAP is refused
/// or writes nothing for [`SILENT_AFTER`] while the display is on, the pixels come in the gRPC messages and this host
/// writes them into the region (at most 30 a second, the "slow" mark); when that fails too, a `getScreenshot` at most
/// 3 times a second (the still badge). A stream that ends by itself after bringing frames is opened again in the same
/// mode after [`stream_retry_wait`], while the device is still attached here and not paused; a pause, a detach or a
/// new region aborts this task instead.
fn restart_stream(host: &Arc<Host>, d: &Arc<Device>) {
    if let Some(s) = lock(&d.stream).take() {
        s.abort();
    }
    if d.paused.load(Ordering::Relaxed) {
        return;
    }
    let Some((url, bytes)) = lock(&d.region).clone() else {
        return;
    };
    let h = Arc::clone(host);
    let dd = Arc::clone(d);
    let task = host.rt.spawn(async move {
        let mut tries = 0;
        let mut via = Via::Mmap;
        loop {
            let ended = match via {
                Via::Mmap | Via::Pixels => {
                    stream(Arc::clone(&h), Arc::clone(&dd), &url, bytes, via).await
                }
                Via::Still => stills(Arc::clone(&h), Arc::clone(&dd), &url, bytes).await,
            };
            let current = h.device(&dd.target).is_some_and(|x| Arc::ptr_eq(&x, &dd));
            if !current || dd.paused.load(Ordering::Relaxed) {
                return;
            }
            if let Some((next, why)) = fall_back(via, &ended) {
                h.log.line(&format!(
                    "{}: frames fall back to {} (emulator {}): {why}",
                    dd.target,
                    next.mode_name(),
                    dd.version
                ));
                via = next;
                *lock(&dd.fell_back) = why.clone();
                tell_mode(&h, &dd, next.mode(), why);
                tries = 0;
                continue;
            }
            tries = if ended == Ended::Frames { 1 } else { tries + 1 };
            let Some(wait) = stream_retry_wait(tries) else {
                h.log.line(&format!(
                    "{}: the frame stream was not opened again after {STREAM_RETRIES} tries",
                    dd.target
                ));
                return;
            };
            h.log.line(&format!(
                "{}: the frame stream opens again in {} s",
                dd.target,
                wait.as_secs()
            ));
            tokio::time::sleep(wait).await;
        }
    });
    *lock(&d.stream) = Some(task);
}

/// How the frames reach Flare's region (the spec's "Frames", in fallback order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Via {
    /// The emulator writes each frame into the region (MMAP).
    Mmap,
    /// The pixels come in each gRPC message; this host writes them into the region, at most 30 a second.
    Pixels,
    /// A `getScreenshot` at most 3 times a second, written into the region by this host.
    Still,
}

impl Via {
    fn mode(self) -> FrameMode {
        match self {
            Via::Mmap => FrameMode::Live,
            Via::Pixels => FrameMode::Slow,
            Via::Still => FrameMode::Still,
        }
    }

    fn mode_name(self) -> &'static str {
        match self {
            Via::Mmap => "the shared memory",
            Via::Pixels => "pixels in the gRPC messages (slow)",
            Via::Still => "still pictures",
        }
    }
}

/// How a frame stream ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ended {
    /// It brought frames, then ended (the guest restarted, a gRPC hiccup): opened again in the same way.
    Frames,
    /// The emulator refused it, or it gave nothing usable for [`SILENT_AFTER`] while the display was on: the next
    /// fallback, and why.
    Failed(String),
}

/// No frame (or a region the emulator never wrote) this long after the stream opened, while the display is on: the
/// stream does not work here (spec "Versions and failure modes").
const SILENT_AFTER: Duration = Duration::from_secs(2);
/// At most this many frames a second on the slow path.
const SLOW_GAP: Duration = Duration::from_micros(1_000_000 / 30);
/// At most this many still pictures a second.
const STILL_GAP: Duration = Duration::from_micros(1_000_000 / 3);

/// The next way to get frames after `via` ended as `ended`, with the reason; None to open the same way again.
fn fall_back(via: Via, ended: &Ended) -> Option<(Via, String)> {
    let Ended::Failed(why) = ended else {
        return None;
    };
    match via {
        Via::Mmap => Some((Via::Pixels, why.clone())),
        Via::Pixels => Some((Via::Still, why.clone())),
        Via::Still => None,
    }
}

/// Says the device's frame mode to Flare, once per change.
fn tell_mode(host: &Host, d: &Device, mode: FrameMode, why: String) {
    {
        let mut told = lock(&d.told);
        if told.0 == mode && told.1 == why {
            return;
        }
        *told = (mode, why.clone());
    }
    host.out.line(&HostLine::Mode {
        target: d.target.clone(),
        mode,
        why,
    });
}

/// After a picture of the display came: it is on, so a dimmed picture brightens and the path's own mode shows (with
/// the reason it fell back, when it did).
fn awake(host: &Host, d: &Device, via: Via) {
    let why = if via == Via::Mmap {
        String::new()
    } else {
        lock(&d.fell_back).clone()
    };
    tell_mode(host, d, via.mode(), why);
}
/// The pixel written at the sample points before MMAP opens: a frame the emulator writes replaces at least one.
const MARK: [u8; 4] = [0x5a, 0xa5, 0x3c, 0xc3];

/// The byte offsets of the 16 pixels sampled across a frame of `n` pixels (the same for every rotation, `w * h`).
fn samples(n: u64) -> impl Iterator<Item = u64> {
    (0..16u64).map(move |i| n.saturating_sub(1) * i / 15 * 4)
}

/// BLOCKING: puts [`MARK`] at the sample points of a frame of `n` pixels, before the MMAP stream opens, so a region
/// the emulator never writes is told from one it wrote (whatever its pixels, alpha included).
fn mark_region(path: &std::path::Path, n: u64) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    for at in samples(n) {
        f.seek(SeekFrom::Start(at))?;
        f.write_all(&MARK)?;
    }
    Ok(())
}

/// BLOCKING: whether the emulator wrote a frame of `w` x `h` into the region since [`mark_region`]: a sample point no
/// longer holds the mark (a fresh read, so what the emulator wrote is seen).
fn region_written_at(path: &std::path::Path, w: u32, h: u32) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    samples(u64::from(w) * u64::from(h)).any(|at| {
        let mut b = [0u8; 4];
        f.seek(SeekFrom::Start(at)).is_ok() && f.read_exact(&mut b).is_ok() && b != MARK
    })
}

/// BLOCKING: writes one frame's pixels into the region (the slow and still paths). The emulator would have written
/// the same bytes there with MMAP: rows top-down, RGBA, `w * 4` bytes a row.
fn write_region(
    path: &std::path::Path,
    bytes: u64,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let need = u64::from(w) * u64::from(h) * 4;
    if need > bytes || (rgba.len() as u64) < need {
        return Err(format!(
            "a {w}x{h} frame of {} bytes does not fit the region of {bytes}",
            rgba.len()
        ));
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(0))
        .and_then(|_| f.write_all(&rgba[..need as usize]))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// One frame's facts.
#[derive(Clone, Copy, Debug)]
struct Fact {
    seq: u64,
    t_us: u64,
    w: u32,
    h: u32,
    rotation: Orientation,
}

/// Frame counts for the once-a-minute log line.
#[derive(Default)]
struct Stats {
    received: AtomicU64,
    sent: AtomicU64,
    gaps: AtomicU64,
    too_big: AtomicU64,
}

/// How many times in a row a frame stream that ended by itself is opened again before the emu host gives up (logged).
const STREAM_RETRIES: u32 = 5;

/// The wait before the `tries`-th new try of a frame stream that ended by itself (1, 2, 4, 8, 16 s), None once
/// [`STREAM_RETRIES`] are used up. A stream that brought frames before it ended starts again at the first try.
fn stream_retry_wait(tries: u32) -> Option<Duration> {
    (1..=STREAM_RETRIES)
        .contains(&tries)
        .then(|| Duration::from_secs(1 << (tries - 1)))
}

/// Keeps the device's orientation from a frame's rotation, and says a turn.
fn follow_turn(host: &Host, d: &Device, rotation: Orientation) {
    let turned = {
        let mut o = lock(&d.orientation);
        let changed = *o != rotation;
        *o = rotation;
        changed
    };
    if turned {
        host.out.line(&HostLine::Orientation {
            target: d.target.clone(),
            orientation: rotation,
        });
    }
}

/// `streamScreenshot` (rule 1363): with MMAP into Flare's region, or (the slow fallback) with the pixels in each
/// message, written into the region here at most 30 a second. One `region` line per frame through the 60 fps cap.
/// A 0x0 frame means the display is off: Flare dims the last picture (spec "Frames").
async fn stream(host: Arc<Host>, d: Arc<Device>, url: &str, bytes: u64, via: Via) -> Ended {
    use pb::image_transport::TransportChannel;
    let path = flare_device::emu::region::path_of_url(url);
    let fmt = pb::ImageFormat {
        format: pb::image_format::ImgFormat::Rgba8888 as i32,
        display: 0,
        transport: (via == Via::Mmap).then(|| pb::ImageTransport {
            channel: TransportChannel::Mmap as i32,
            handle: url.to_string(),
        }),
        ..Default::default()
    };
    if via == Via::Mmap
        && let Some(p) = path.clone()
    {
        // The sample points get the mark, so the first frame shows whether the emulator really writes the region.
        let n = (d.portrait.w * d.portrait.h) as u64;
        if let Err(e) = tokio::task::spawn_blocking(move || mark_region(&p, n))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())))
        {
            host.log.line(&format!(
                "{}: the region could not be marked: {e}",
                d.target
            ));
        }
    }
    let mut s = match d.client.clone().stream_screenshot(fmt).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            let why = format!(
                "the frame stream was refused ({} {})",
                e.code(),
                e.message()
            );
            host.log.line(&format!("{}: {why}", d.target));
            return Ended::Failed(why);
        }
    };
    host.log.line(&format!(
        "{}: frames streaming ({})",
        d.target,
        via.mode_name()
    ));
    let mut opened = Instant::now();
    let stats = Stats::default();
    let mut cap: FrameCap<Fact> = FrameCap::default();
    let mut last_seq: Option<u64> = None;
    let mut last_log = Instant::now();
    // Whether a frame was seen in the region (MMAP) or written into it (slow): the stream works.
    let mut proven = false;
    // The display was off when the stream opened: no frame is owed until it wakes.
    let mut asleep = false;
    let mut last_write: Option<Instant> = None;
    let mut pending: Option<(Fact, Vec<u8>)> = None;
    let send = |f: Fact| {
        stats.sent.fetch_add(1, Ordering::Relaxed);
        host.out.line(&HostLine::Region {
            target: d.target.clone(),
            seq: f.seq,
            t_us: f.t_us,
            w: f.w,
            h: f.h,
            rotation: f.rotation,
        });
    };
    // The slow path: the newest pixels go into the region and out as a record, at most 30 a second.
    let write = |f: Fact, rgba: Vec<u8>| -> bool {
        let Some(p) = path.clone() else {
            return false;
        };
        match write_region(&p, bytes, &rgba, f.w, f.h) {
            Ok(()) => {
                send(f);
                true
            }
            Err(e) => {
                host.log.line(&format!("{}: {e}", d.target));
                false
            }
        }
    };
    loop {
        let due = cap
            .due_us()
            .map(|us| host.epoch + Duration::from_micros(us));
        let slow_due = pending.as_ref().and(last_write).map(|t| t + SLOW_GAP);
        let silent_at = (!proven && !asleep).then(|| opened + SILENT_AFTER);
        let wake = [due, slow_due, silent_at].into_iter().flatten().min();
        let msg = tokio::select! {
            m = s.message() => m,
            () = async {
                match wake {
                    Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                    None => std::future::pending().await,
                }
            } => {
                if !proven && !asleep && opened.elapsed() >= SILENT_AFTER {
                    let why = if stats.received.load(Ordering::Relaxed) == 0 {
                        "the emulator sent no frame for 2 s".to_string()
                    } else {
                        "the emulator wrote no frame into the shared memory for 2 s".to_string()
                    };
                    host.log.line(&format!("{}: {why}", d.target));
                    return Ended::Failed(why);
                }
                if let Some(f) = cap.poll(host.now_us()) {
                    send(f);
                }
                if last_write.is_some_and(|t| t.elapsed() >= SLOW_GAP)
                    && let Some((f, rgba)) = pending.take()
                    && tokio::task::block_in_place(|| write(f, rgba))
                {
                    last_write = Some(Instant::now());
                }
                continue;
            }
        };
        let img = match msg {
            Ok(Some(i)) => i,
            Ok(None) => {
                host.log
                    .line(&format!("{}: the frame stream ended", d.target));
                break;
            }
            Err(e) => {
                host.log.line(&format!(
                    "{}: the frame stream failed: {} {}",
                    d.target,
                    e.code(),
                    e.message()
                ));
                break;
            }
        };
        stats.received.fetch_add(1, Ordering::Relaxed);
        let seq = u64::from(img.seq);
        if last_seq.is_some_and(|l| seq > l + 1) {
            stats.gaps.fetch_add(1, Ordering::Relaxed);
        }
        last_seq = Some(seq);
        let f = img.format.unwrap_or_default();
        // A 0x0 frame: the display is off. The last picture stays, dimmed; nothing is owed until it wakes.
        if f.width == 0 || f.height == 0 {
            asleep = true;
            tell_mode(&host, &d, FrameMode::Asleep, String::new());
            continue;
        }
        if u64::from(f.width) * u64::from(f.height) * 4 > bytes {
            stats.too_big.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if asleep {
            // Awake again: the stream owes a frame within SILENT_AFTER from now.
            asleep = false;
            opened = Instant::now();
        }
        let rotation = keys::orientation_of(f.rotation.map_or(0, |r| r.rotation));
        follow_turn(&host, &d, rotation);
        let fact = Fact {
            seq,
            t_us: img.timestamp_us,
            w: f.width,
            h: f.height,
            rotation,
        };
        match via {
            Via::Mmap => {
                if !proven {
                    // The emulator says a frame is there: is it? (An emulator whose MMAP is broken acks frames and
                    // writes nothing.)
                    let p = path.clone();
                    let (w, h) = (f.width, f.height);
                    proven = tokio::task::spawn_blocking(move || {
                        p.is_some_and(|p| region_written_at(&p, w, h))
                    })
                    .await
                    .unwrap_or(false);
                    if !proven {
                        continue;
                    }
                }
                awake(&host, &d, via);
                if let Some(f) = cap.offer(fact, host.now_us()) {
                    send(f);
                }
            }
            Via::Pixels => {
                if img.image.is_empty() {
                    continue;
                }
                awake(&host, &d, via);
                if last_write.is_none_or(|t| t.elapsed() >= SLOW_GAP) {
                    if tokio::task::block_in_place(|| write(fact, img.image)) {
                        proven = true;
                        last_write = Some(Instant::now());
                    }
                } else {
                    pending = Some((fact, img.image));
                }
            }
            Via::Still => {}
        }
        if last_log.elapsed() >= STATS_EVERY {
            last_log = Instant::now();
            host.log.line(&format!(
                "{}: frames received {}, sent {}, capped {}, seq gaps {}, too big {} ({})",
                d.target,
                stats.received.load(Ordering::Relaxed),
                stats.sent.load(Ordering::Relaxed),
                cap.capped(),
                stats.gaps.load(Ordering::Relaxed),
                stats.too_big.load(Ordering::Relaxed),
                via.mode_name()
            ));
        }
    }
    if proven {
        Ended::Frames
    } else {
        Ended::Failed("the frame stream ended before its first frame".to_string())
    }
}

/// The last fallback: a `getScreenshot` (RGBA) at most 3 times a second, written into the region here, with the still
/// badge. Runs until the device is paused or detached (the task is aborted) or the screenshots fail.
async fn stills(host: Arc<Host>, d: Arc<Device>, url: &str, bytes: u64) -> Ended {
    let Some(path) = flare_device::emu::region::path_of_url(url) else {
        return Ended::Failed(format!("no region file in {url:?}"));
    };
    let mut seq: u64 = 0;
    let mut fails = 0;
    loop {
        let started = Instant::now();
        let r = d
            .client
            .clone()
            .get_screenshot(pb::ImageFormat {
                format: pb::image_format::ImgFormat::Rgba8888 as i32,
                display: 0,
                ..Default::default()
            })
            .await;
        match r {
            Err(e) => {
                fails += 1;
                host.log.line(&format!(
                    "{}: a still picture failed ({fails}): {} {}",
                    d.target,
                    e.code(),
                    e.message()
                ));
                if fails >= 3 {
                    return Ended::Failed(format!("no still picture either: {}", e.message()));
                }
            }
            Ok(img) => {
                fails = 0;
                let img = img.into_inner();
                let f = img.format.unwrap_or_default();
                if f.width == 0 || f.height == 0 {
                    tell_mode(&host, &d, FrameMode::Asleep, String::new());
                } else {
                    let rotation = keys::orientation_of(f.rotation.map_or(0, |r| r.rotation));
                    follow_turn(&host, &d, rotation);
                    let p = path.clone();
                    let (w, h) = (f.width, f.height);
                    let pixels = img.image;
                    let wrote =
                        tokio::task::spawn_blocking(move || write_region(&p, bytes, &pixels, w, h))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                    match wrote {
                        Ok(()) => {
                            awake(&host, &d, Via::Still);
                            seq += 1;
                            host.out.line(&HostLine::Region {
                                target: d.target.clone(),
                                seq,
                                t_us: host.now_us(),
                                w,
                                h,
                                rotation,
                            });
                        }
                        Err(e) => host.log.line(&format!("{}: {e}", d.target)),
                    }
                }
            }
        }
        tokio::time::sleep(STILL_GAP.saturating_sub(started.elapsed())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1368: nothing in the emu host reaches the computer's clipboard; its only clipboard is the device's own
    /// (gRPC `setClipboard`). The needles are split so this file does not match.
    #[test]
    fn the_emu_host_never_touches_the_computers_clipboard() {
        let needles = [
            concat!("NS", "Pasteboard"),
            concat!("pb", "copy"),
            concat!("Open", "Clipboard"),
            concat!("SetClipboard", "Data"),
            concat!("arb", "oard"),
            concat!("clip", ".exe"),
        ];
        for (name, text) in crate::run::tests::sources() {
            for n in needles {
                assert!(!text.contains(n), "{name} uses {n}");
            }
        }
        let typed = keys::text_steps("bl\u{e5}b\u{e6}r");
        assert!(matches!(&typed[0], keys::TextStep::DeviceClipboard(_)));
    }

    /// The spec's fallbacks, in order (PR A7): a refused or silent MMAP stream goes to pixels in the gRPC messages
    /// (slow), and that to still pictures; a stream that brought frames is opened again the same way.
    #[test]
    fn a_failed_stream_falls_back_in_the_specs_order() {
        let failed = Ended::Failed("refused".into());
        assert_eq!(
            fall_back(Via::Mmap, &failed),
            Some((Via::Pixels, "refused".to_string()))
        );
        assert_eq!(
            fall_back(Via::Pixels, &failed),
            Some((Via::Still, "refused".to_string()))
        );
        assert_eq!(fall_back(Via::Still, &failed), None, "retried with waits");
        for via in [Via::Mmap, Via::Pixels, Via::Still] {
            assert_eq!(fall_back(via, &Ended::Frames), None);
        }
        assert_eq!(
            [Via::Mmap, Via::Pixels, Via::Still].map(Via::mode),
            [FrameMode::Live, FrameMode::Slow, FrameMode::Still]
        );
        assert_eq!(SLOW_GAP, Duration::from_micros(33_333), "30 a second");
        assert_eq!(STILL_GAP, Duration::from_micros(333_333), "3 a second");
        assert_eq!(SILENT_AFTER, Duration::from_secs(2));
    }

    /// The slow and still paths write a frame into the region as the emulator would (rows top-down, RGBA), and only
    /// a frame that fits; the MMAP check tells a region nobody wrote since the mark from one the emulator wrote.
    #[test]
    fn frames_are_written_into_the_region_and_a_blank_region_is_seen() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("1-1.rgba");
        let bytes = 64 * 1024;
        std::fs::write(&p, vec![0u8; bytes]).unwrap();
        mark_region(&p, 100 * 100).unwrap();
        assert!(
            !region_written_at(&p, 100, 100),
            "never written since the mark"
        );
        // A frame of the same pixel count in another rotation samples the same points.
        assert!(!region_written_at(&p, 50, 200));
        let mut px = vec![0u8; 100 * 100 * 4];
        for (i, c) in px.chunks_mut(4).enumerate() {
            c.copy_from_slice(&[(i % 251) as u8, 1, 2, 255]);
        }
        write_region(&p, bytes as u64, &px, 100, 100).unwrap();
        let got = std::fs::read(&p).unwrap();
        assert_eq!(&got[..px.len()], &px[..]);
        assert_eq!(got.len(), bytes, "the region keeps its size");
        assert!(region_written_at(&p, 100, 100));
        // A black, fully transparent frame counts as written too: only the mark says "not written".
        write_region(&p, bytes as u64, &vec![0u8; 100 * 100 * 4], 100, 100).unwrap();
        assert!(region_written_at(&p, 100, 100));
        // Too big for the region, or fewer pixels than the size says: refused, nothing written.
        assert!(write_region(&p, bytes as u64, &px, 200, 200).is_err());
        assert!(write_region(&p, bytes as u64, &px[..10], 100, 100).is_err());
        assert!(!region_written_at(&d.path().join("none.rgba"), 1, 1));
    }

    /// A frame stream that ends by itself is opened again, waiting longer each time, and given up after
    /// STREAM_RETRIES tries in a row with no frame.
    #[test]
    fn a_frame_stream_that_ends_is_opened_again_a_few_times() {
        let waits: Vec<u64> = (1..=STREAM_RETRIES)
            .map(|t| stream_retry_wait(t).expect("a wait").as_secs())
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16]);
        assert_eq!(stream_retry_wait(0), None);
        assert_eq!(stream_retry_wait(STREAM_RETRIES + 1), None);
    }

    /// Rule 1362: the hello says the host's kind and facts, and no token.
    #[test]
    fn the_hello_names_the_emu_host_and_its_facts() {
        let s = Sdk {
            root: "C:/sdk".into(),
            flare_own: false,
            emulator_version: "37.2.13".into(),
        };
        let a = flare_device::emu::accel::parse(
            "accel:\n0\nWHPX(10.0.26300) is installed and usable.\naccel\n",
        );
        let h = hello(&Found::Ready(s), a.as_ref());
        assert_eq!(h.host, HostKind::Emu);
        assert_eq!(h.protocol, PROTOCOL);
        assert_eq!(h.facts["sdk"], "ready");
        assert_eq!(h.facts["emulator_version"], "37.2.13");
        assert_eq!(h.facts["accel"], "whpx");
        // The jar's short hash (a test build finds the repo's jar).
        assert_eq!(
            h.facts["agent_jar"],
            &flare_device::emu::agent::jar_sha256()[..12]
        );
        assert!(h.input_path.is_none());
        let line = to_line(&HostLine::Hello(h));
        assert!(!line.contains("token"), "{line}");
        assert_eq!(hello(&Found::Missing, None).facts["sdk"], "missing");
    }

    #[test]
    fn keys_and_touches_become_the_emulators_events() {
        let k = key_event(Key {
            usage: 0x04,
            down: true,
        });
        let Some(pb::input_event::Type::KeyEvent(e)) = k.r#type else {
            panic!()
        };
        assert_eq!(e.key_code, 0x07_0004);
        assert_eq!(e.code_type, pb::keyboard_event::KeyCodeType::Usb as i32);
        assert_eq!(
            e.event_type,
            pb::keyboard_event::KeyEventType::Keydown as i32
        );
        let t = touch_event(vec![pb::Touch {
            x: 1,
            y: 2,
            identifier: 0,
            pressure: PRESSURE,
            ..Default::default()
        }]);
        let Some(pb::input_event::Type::TouchEvent(e)) = t.r#type else {
            panic!()
        };
        assert_eq!((e.touches[0].x, e.touches[0].pressure), (1, PRESSURE));
    }
}
