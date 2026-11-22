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
    Button, DeviceInfo, DeviceState, InputState, Key, Phase, Screen, Touch,
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
    // The agent helper comes with Android plan PR A3.
    facts.insert("agent_jar".to_string(), "none".to_string());
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
        Request::UiTree { target, .. } => {
            host.err(
                id,
                Fail::Unavailable,
                format!("{target}: the UI tree comes with the agent helper (Android plan PR A3)"),
            );
        }
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
    Ok(())
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

/// Types `text` (rule 1368): HID keys, or the device's own clipboard and Ctrl+V. Never the computer's clipboard.
async fn type_text(d: &Device, text: &str) -> Result<(), Failed> {
    for step in keys::text_steps(text) {
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
/// (Re)starts the frame stream when the device has a region and is not paused (rule 1365). A stream that ends by
/// itself (the guest restarted, a gRPC hiccup) is opened again after [`stream_retry_wait`], while the device is still
/// attached here and not paused; a pause, a detach or a new region aborts this task instead.
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
        loop {
            let got = stream(Arc::clone(&h), Arc::clone(&dd), url.clone(), bytes).await;
            let current = h.device(&dd.target).is_some_and(|x| Arc::ptr_eq(&x, &dd));
            if !current || dd.paused.load(Ordering::Relaxed) {
                return;
            }
            tries = if got { 1 } else { tries + 1 };
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

/// `streamScreenshot` with MMAP into Flare's region (rule 1363); one `region` line per frame through the 60 fps cap.
/// Returns whether any frame came before the stream ended (a refused stream brings none).
async fn stream(host: Arc<Host>, d: Arc<Device>, url: String, bytes: u64) -> bool {
    use pb::image_transport::TransportChannel;
    let fmt = pb::ImageFormat {
        format: pb::image_format::ImgFormat::Rgba8888 as i32,
        display: 0,
        transport: Some(pb::ImageTransport {
            channel: TransportChannel::Mmap as i32,
            handle: url,
        }),
        ..Default::default()
    };
    let mut s = match d.client.clone().stream_screenshot(fmt).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            host.log.line(&format!(
                "{}: the frame stream was refused: {} {}",
                d.target,
                e.code(),
                e.message()
            ));
            return false;
        }
    };
    host.log.line(&format!("{}: frames streaming", d.target));
    let stats = Stats::default();
    let mut cap: FrameCap<Fact> = FrameCap::default();
    let mut last_seq: Option<u64> = None;
    let mut last_log = Instant::now();
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
    loop {
        let due = cap
            .due_us()
            .map(|us| host.epoch + Duration::from_micros(us));
        let msg = tokio::select! {
            m = s.message() => m,
            () = async {
                match due {
                    Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(f) = cap.poll(host.now_us()) {
                    send(f);
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
        // A 0x0 frame: the display is off. Nothing to show.
        if f.width == 0 || f.height == 0 {
            continue;
        }
        if u64::from(f.width) * u64::from(f.height) * 4 > bytes {
            stats.too_big.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let rotation = keys::orientation_of(f.rotation.map_or(0, |r| r.rotation));
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
        let fact = Fact {
            seq,
            t_us: img.timestamp_us,
            w: f.width,
            h: f.height,
            rotation,
        };
        if let Some(f) = cap.offer(fact, host.now_us()) {
            send(f);
        }
        if last_log.elapsed() >= STATS_EVERY {
            last_log = Instant::now();
            host.log.line(&format!(
                "{}: frames received {}, sent {}, capped {}, seq gaps {}, too big {}",
                d.target,
                stats.received.load(Ordering::Relaxed),
                stats.sent.load(Ordering::Relaxed),
                cap.capped(),
                stats.gaps.load(Ordering::Relaxed),
                stats.too_big.load(Ordering::Relaxed)
            ));
        }
    }
    stats.received.load(Ordering::Relaxed) > 0
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
        assert_eq!(h.facts["agent_jar"], "none");
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
