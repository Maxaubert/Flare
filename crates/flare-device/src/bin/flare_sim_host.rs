//! flare-sim-host: Flare's helper for the device panel (issue #248). The only process that loads CoreSimulator and
//! SimulatorKit (rule 1310); one per Flare, started guarded so it dies with Flare (rule 1312).
//!
//! Modes:
//! - `--rendezvous <service> [--log <file>] [--probe-barcode]`: serve Flare. Control comes as JSON lines on stdin
//!   (`flare_device::wire::Control`), answers and events go as JSON lines on stdout (`HostLine`, a `hello` first).
//!   Frames go over the mach channel the service name sets up. The log is `<profile>/logs/sim-host.log` (1 MB x 3).
//!   It serves the frames, the device list and `simctl` (PR 2), and input, rotation, the UI tree and snapshots
//!   (PR 3). `--restarted` says Flare started it again after one ended (any finger the old one left down is ended).
//! - `--self-test`: load the frameworks and check every class and selector; JSON out, exit 0 when they are all there
//!   (or when there is no Xcode to test, with the reason).
//! - `--bench <udid|new> <seconds> [--heavy] [--probe-app <path>] [--late-ms <n>]`: the frame path against the probe
//!   app; JSON out.
//! - `--input-bench <udid|new> [--probe-app <path>] [--taps <n>] [--swipes <n>] [--chars <n>]`: the input path
//!   against the touch probe app (taps, swipes, typed characters, landscape taps), latency from the app's painted
//!   frame; JSON out.
//!
//! Everything runs headless: no window, no Simulator.app, no permission prompt.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-sim-host: macOS only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(mac::main());
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use flare_device::geom::Orientation;
    use flare_device::job::Fail;
    use flare_device::macos::ax::{self, Ax, Serial};
    use flare_device::macos::bench::{self, BenchArgs, InputBenchArgs};
    use flare_device::macos::indigo::PathLock;
    use flare_device::macos::input::{self, Cmd, Input};
    use flare_device::macos::load;
    use flare_device::macos::orient;
    use flare_device::macos::rendezvous::{Link, RecvError};
    use flare_device::macos::ring::{Gpu, GpuRing};
    use flare_device::macos::sim::{self, AttachOptions, Attached, Sim, Sink, Stream};
    use flare_device::macos::simctl::Simctl;
    use flare_device::macos::snapshot;
    use flare_device::source::{InputState, Screen};
    use flare_device::target::Target;
    use flare_device::wire::{
        Control, HostLine, PROTOCOL, Record, Request, from_line, png_result, to_line, tree_result,
    };
    use serde_json::{Value, json};

    /// The log's size before it rotates, and the copies kept.
    const LOG_MAX: u64 = 1024 * 1024;
    const LOG_KEEP: usize = 3;
    /// How often the stream counts go to the log.
    const STATS_EVERY: Duration = Duration::from_secs(60);

    /// `sim-host.log`, rotated at [`LOG_MAX`] (`.1`, `.2` kept).
    struct HostLog {
        path: Option<PathBuf>,
        file: Option<File>,
        size: u64,
    }

    impl HostLog {
        fn open(path: Option<PathBuf>) -> HostLog {
            let file = path.as_ref().and_then(|p| {
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                OpenOptions::new().create(true).append(true).open(p).ok()
            });
            let size = file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map_or(0, |m| m.len());
            HostLog { path, file, size }
        }

        fn line(&mut self, text: &str) {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let l = format!("{secs} [{}] {text}\n", std::process::id());
            let Some(f) = self.file.as_mut() else {
                eprint!("{l}");
                return;
            };
            let _ = f.write_all(l.as_bytes());
            self.size += l.len() as u64;
            if self.size >= LOG_MAX
                && let Some(p) = self.path.clone()
            {
                self.file = None;
                for i in (1..LOG_KEEP).rev() {
                    let from = if i == 1 {
                        p.clone()
                    } else {
                        p.with_extension(format!("log.{}", i - 1))
                    };
                    let _ = std::fs::rename(&from, p.with_extension(format!("log.{i}")));
                }
                *self = HostLog::open(Some(p));
            }
        }
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let flag = |name: &str| args.iter().any(|a| a == name);
        let value = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        if flag("--self-test") {
            let r = load::run_self_test();
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            return if r.ok { 0 } else { 1 };
        }
        if let Some(i) = args.iter().position(|a| a == "--bench") {
            let (Some(device), Some(seconds)) = (
                args.get(i + 1).cloned(),
                args.get(i + 2).and_then(|s| s.parse::<f64>().ok()),
            ) else {
                eprintln!(
                    "usage: flare-sim-host --bench <udid|new> <seconds> [--heavy] [--probe-app <path>] [--late-ms <n>]"
                );
                return 2;
            };
            let bench_args = BenchArgs {
                device,
                seconds: seconds.clamp(1.0, 600.0),
                heavy: flag("--heavy"),
                probe_app: value("--probe-app").map(PathBuf::from),
                late_ms: value("--late-ms")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(17),
            };
            let dev = match load::developer_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("flare-sim-host: {e}");
                    return 3;
                }
            };
            if let Err(e) = load::load(&dev) {
                eprintln!("flare-sim-host: {e}");
                return 3;
            }
            return match bench::run(&bench_args, &dev) {
                Ok(r) => {
                    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("flare-sim-host: bench: {e}");
                    1
                }
            };
        }
        if let Some(i) = args.iter().position(|a| a == "--input-bench") {
            let Some(device) = args.get(i + 1).cloned() else {
                eprintln!(
                    "usage: flare-sim-host --input-bench <udid|new> [--probe-app <path>] [--taps <n>] [--swipes <n>] [--chars <n>]"
                );
                return 2;
            };
            let n = |name: &str, d: usize| {
                value(name)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(d)
            };
            let bench_args = InputBenchArgs {
                device,
                probe_app: value("--probe-app").map(PathBuf::from),
                taps: n("--taps", 100),
                swipes: n("--swipes", 20),
                chars: n("--chars", 100),
                landscape_taps: n("--landscape-taps", 10),
            };
            let dev = match load::developer_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("flare-sim-host: {e}");
                    return 3;
                }
            };
            let loaded = match load::load(&dev) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("flare-sim-host: {e}");
                    return 3;
                }
            };
            return match bench::run_input(&bench_args, &dev, &loaded.coresim_version) {
                Ok(r) => {
                    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    i32::from(!r.passed)
                }
                Err(e) => {
                    eprintln!("flare-sim-host: input bench: {e}");
                    1
                }
            };
        }
        if let Some(name) = value("--rendezvous") {
            return serve(
                &name,
                value("--log").map(PathBuf::from),
                flag("--probe-barcode"),
                flag("--restarted"),
            );
        }
        eprintln!(
            "usage: flare-sim-host --rendezvous <service> [--log <file>] [--probe-barcode] | --self-test | --bench <udid|new> <seconds>"
        );
        2
    }

    /// A UI tree request: the device's token, a point or none, and who to answer (None: the warm-up after attach).
    struct AxJob {
        token: String,
        at: Option<(f64, f64)>,
        reply: Option<(u64, std::sync::Weak<Host>)>,
    }

    /// The sim host serving Flare.
    struct Host {
        out: Mutex<std::io::Stdout>,
        log: Mutex<HostLog>,
        link: Arc<Link>,
        sim: Sim,
        gpu: Arc<Gpu>,
        ids: Arc<AtomicU32>,
        simctl: Simctl,
        probe_barcode: bool,
        /// This sim host replaced one that ended.
        restarted: bool,
        coresim_version: String,
        attached: Mutex<HashMap<Target, Attached>>,
        streams: Mutex<HashMap<Target, Arc<Stream>>>,
        devices: Mutex<HashMap<u32, Target>>,
        records_dropped: AtomicU64,
        /// Each attached device's input worker and interface orientation.
        inputs: Mutex<HashMap<Target, Arc<Input>>>,
        orientations: Mutex<HashMap<Target, Arc<Mutex<Orientation>>>>,
        /// The input path of each boot (never mixed, rule 1313).
        paths: Mutex<PathLock>,
        /// The UI tree, one request in flight (None without AccessibilityPlatformTranslation).
        ax: Option<Serial<AxJob>>,
    }

    /// The input workers' answers and news.
    struct HostOut(std::sync::Weak<Host>);

    impl input::Out for HostOut {
        fn answer(&self, id: u64, r: Result<(), (Fail, String)>) {
            if let Some(h) = self.0.upgrade() {
                match r {
                    Ok(()) => h.ok(id, Value::Null),
                    Err((f, m)) => h.err(id, f, m),
                }
            }
        }

        fn input(&self, target: &Target, state: InputState) {
            if let Some(h) = self.0.upgrade() {
                h.write(&HostLine::Input {
                    target: target.clone(),
                    state,
                });
            }
        }

        fn orientation(&self, target: &Target, orientation: Orientation) {
            if let Some(h) = self.0.upgrade() {
                h.write(&HostLine::Orientation {
                    target: target.clone(),
                    orientation,
                });
            }
        }

        fn log(&self, line: &str) {
            if let Some(h) = self.0.upgrade() {
                h.log(line);
            }
        }
    }

    impl Host {
        fn write(&self, line: &HostLine) {
            if let Ok(mut o) = self.out.lock() {
                let _ = writeln!(o, "{}", to_line(line));
                let _ = o.flush();
            }
        }

        fn log(&self, text: &str) {
            if let Ok(mut l) = self.log.lock() {
                l.line(text);
            }
        }

        fn ok(&self, id: u64, result: Value) {
            self.write(&HostLine::Ok { id, result });
        }

        fn err(&self, id: u64, fail: Fail, message: impl Into<String>) {
            self.write(&HostLine::Err {
                id,
                code: fail.code().to_string(),
                message: message.into(),
            });
        }
    }

    /// The frames' way out: rings over mach and a `frames` line, records over mach.
    struct HostSink(std::sync::Weak<Host>);

    impl Sink for HostSink {
        fn ring(
            &self,
            target: &Target,
            device: u32,
            ring: u32,
            screen: Screen,
            gpu_ring: &Arc<GpuRing>,
        ) {
            let Some(h) = self.0.upgrade() else {
                return;
            };
            if let Ok(mut d) = h.devices.lock() {
                d.insert(device, target.clone());
            }
            let surfaces: Vec<&objc2_io_surface::IOSurfaceRef> =
                gpu_ring.surfaces.iter().map(|s| &**s).collect();
            if let Err(kr) = h.link.send_ring(device, ring, &surfaces) {
                h.log(&format!(
                    "{target}: the ring did not reach Flare (kern_return {kr:#x})"
                ));
                return;
            }
            h.write(&HostLine::Frames {
                target: target.clone(),
                device,
                ring,
                screen,
            });
            h.log(&format!(
                "{target}: ring {ring} (device {device}) {}x{} px, scale {}",
                gpu_ring.width, gpu_ring.height, screen.scale
            ));
        }

        fn record(&self, record: Record) -> bool {
            let Some(h) = self.0.upgrade() else {
                return false;
            };
            let sent = h.link.send_record(&record);
            if !sent {
                h.records_dropped.fetch_add(1, Ordering::Relaxed);
            }
            sent
        }

        fn log(&self, line: &str) {
            if let Some(h) = self.0.upgrade() {
                h.log(line);
            }
        }
    }

    fn broken(out: &mut std::io::Stdout, log: &mut HostLog, why: &str) {
        log.line(&format!("broken: {why}"));
        let _ = writeln!(
            out,
            "{}",
            to_line(&HostLine::Broken {
                why: why.to_string()
            })
        );
        let _ = out.flush();
    }

    fn serve(name: &str, log_path: Option<PathBuf>, probe_barcode: bool, restarted: bool) -> i32 {
        let mut log = HostLog::open(log_path);
        let mut out = std::io::stdout();
        log.line(&format!(
            "flare-sim-host {} starts, service {name}",
            env!("CARGO_PKG_VERSION")
        ));
        let dev = match load::developer_dir() {
            Ok(d) => d,
            Err(e) => {
                broken(&mut out, &mut log, &e);
                return 3;
            }
        };
        let loaded = match load::load(&dev) {
            Ok(l) => l,
            Err(e) => {
                broken(&mut out, &mut log, &e);
                return 3;
            }
        };
        let hello = loaded.hello();
        log.line(&format!(
            "Xcode {} at {}, CoreSimulator {}, SimulatorKit {} ({}), input {:?}",
            hello.xcode_version,
            hello.xcode_path,
            hello.coresim_version,
            hello.simkit_version,
            loaded.simkit_path.display(),
            loaded.input_path()
        ));
        let missing = load::self_test();
        if let Some(b) = missing.first() {
            broken(&mut out, &mut log, &b.in_coresim(&hello.coresim_version));
            return 3;
        }
        let link = match Link::connect(name, PROTOCOL) {
            Ok(l) => Arc::new(l),
            Err(e) => {
                broken(
                    &mut out,
                    &mut log,
                    &format!("the ring handover failed: {e}"),
                );
                return 4;
            }
        };
        let sim = match Sim::open(&dev) {
            Ok(s) => s,
            Err(b) => {
                broken(&mut out, &mut log, &b.in_coresim(&hello.coresim_version));
                return 3;
            }
        };
        let gpu = match Gpu::new() {
            Ok(g) => Arc::new(g),
            Err(e) => {
                broken(&mut out, &mut log, &e);
                return 3;
            }
        };
        // The UI tree: its worker owns the translator. Missing, the sim host runs with no tree.
        let ax = if hello.ax {
            match Ax::open() {
                Ok(a) => Some(a),
                Err(b) => {
                    log.line(&format!("no UI tree: {b}"));
                    None
                }
            }
        } else {
            log.line("no UI tree: AccessibilityPlatformTranslation or its selectors are missing");
            None
        };
        let mut hello = hello;
        hello.ax = ax.is_some();
        let _ = writeln!(out, "{}", to_line(&HostLine::Hello(hello.clone())));
        let _ = out.flush();
        let ax = ax.map(|a| {
            Serial::start("ax", move |j: AxJob| {
                let started = Instant::now();
                let r = a.tree(&j.token, j.at);
                let Some((id, host)) = j.reply else {
                    return;
                };
                if let Some(h) = host.upgrade() {
                    match r {
                        Ok(tree) => {
                            h.log(&format!("ui tree in {} ms", started.elapsed().as_millis()));
                            h.ok(id, tree_result(&tree));
                        }
                        Err(b) => h.err(id, Fail::Unavailable, b.to_string()),
                    }
                }
            })
        });
        let host = Arc::new(Host {
            out: Mutex::new(out),
            log: Mutex::new(log),
            link,
            sim,
            gpu,
            ids: Arc::new(AtomicU32::new(1)),
            simctl: Simctl::new(&dev),
            probe_barcode,
            restarted,
            coresim_version: hello.coresim_version.clone(),
            attached: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            records_dropped: AtomicU64::new(0),
            inputs: Mutex::new(HashMap::new()),
            orientations: Mutex::new(HashMap::new()),
            paths: Mutex::new(PathLock::default()),
            ax,
        });
        spawn_slot_done(Arc::clone(&host));
        spawn_stats(Arc::downgrade(&host));
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Some(c) = from_line::<Control>(&line) else {
                host.log(&format!(
                    "refused a control line: {}",
                    line.chars().take(200).collect::<String>()
                ));
                continue;
            };
            if !handle(&host, c) {
                break;
            }
        }
        // Flare said quit, or its end of the pipe closed: every callback is undone.
        detach_all(&host);
        host.log("ends");
        0
    }

    fn detach_all(host: &Host) {
        let all: Vec<(Target, Attached)> = host
            .attached
            .lock()
            .map(|mut a| a.drain().collect())
            .unwrap_or_default();
        for (t, a) in all {
            forget_device(host, &t);
            a.detach();
        }
        if let Ok(mut s) = host.streams.lock() {
            s.clear();
        }
    }

    /// Stops a device's input worker (it ends any finger down) and forgets its UI tree token.
    fn forget_device(host: &Host, target: &Target) {
        if let Ok(mut i) = host.inputs.lock() {
            i.remove(target);
        }
        if let Ok(mut o) = host.orientations.lock() {
            o.remove(target);
        }
        ax::forget(target.id());
    }

    fn input_of(host: &Host, target: &Target) -> Option<Arc<Input>> {
        host.inputs.lock().ok().and_then(|i| i.get(target).cloned())
    }

    /// Flare's slot-done records free ring slots (only Flare's pid is heard).
    fn spawn_slot_done(host: Arc<Host>) {
        std::thread::spawn(move || {
            loop {
                match host.link.recv(Duration::from_millis(500)) {
                    Ok(Record::SlotDone(d)) => {
                        let find = || {
                            host.devices
                                .lock()
                                .ok()
                                .and_then(|m| m.get(&d.device).cloned())
                                .and_then(|t| {
                                    host.streams.lock().ok().and_then(|s| s.get(&t).cloned())
                                })
                        };
                        // A record for a ring whose attach is still returning: it is known in a moment.
                        let mut stream = find();
                        for _ in 0..20 {
                            if stream.is_some() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                            stream = find();
                        }
                        if let Some(s) = stream {
                            s.slot_done(d.device, d.slot);
                        }
                    }
                    Ok(_) | Err(RecvError::TimedOut) => {}
                    Err(RecvError::Refused { pid, why }) => {
                        host.log(&format!("refused {why} from pid {pid}"));
                    }
                    Err(RecvError::Mach(kr)) => {
                        host.log(&format!("the slot-done port failed: kern_return {kr:#x}"));
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        });
    }

    /// Once a minute while live: frames received, sent, dropped (no free slot) and capped, per device.
    fn spawn_stats(host: std::sync::Weak<Host>) {
        std::thread::spawn(move || {
            let mut last: HashMap<Target, sim::Stats> = HashMap::new();
            loop {
                std::thread::sleep(STATS_EVERY);
                let Some(h) = host.upgrade() else {
                    return;
                };
                let streams: Vec<Arc<Stream>> = h
                    .streams
                    .lock()
                    .map(|s| s.values().cloned().collect())
                    .unwrap_or_default();
                for s in streams {
                    let now = s.stats();
                    let before = last.insert(s.target.clone(), now).unwrap_or_default();
                    let mut line = format!(
                        "{}: last minute {} frames, {} sent, {} dropped (no free slot), {} capped",
                        s.target,
                        now.callbacks - before.callbacks,
                        now.sent - before.sent,
                        now.dropped - before.dropped,
                        now.capped - before.capped
                    );
                    if h.probe_barcode
                        && let Some(code) = s.last_barcode()
                    {
                        line.push_str(&format!(", probe frame {code}"));
                    }
                    h.log(&line);
                }
                let dropped = h.records_dropped.load(Ordering::Relaxed);
                if dropped > 0 {
                    h.log(&format!("{dropped} records found Flare's port full"));
                }
            }
        });
    }

    /// Runs a `simctl` call off the control loop and answers it.
    fn on_worker(
        host: &Arc<Host>,
        id: u64,
        work: impl FnOnce(&Host) -> Result<Value, (Fail, String)> + Send + 'static,
    ) {
        let h = Arc::clone(host);
        std::thread::spawn(move || match work(&h) {
            Ok(v) => h.ok(id, v),
            Err((f, m)) => h.err(id, f, m),
        });
    }

    /// One control line. False to end (quit).
    fn handle(host: &Arc<Host>, c: Control) -> bool {
        let id = c.id;
        match c.req {
            Request::Hello { protocol } => {
                if protocol != PROTOCOL {
                    host.err(
                        id,
                        Fail::Unavailable,
                        format!("protocol {protocol}, this sim host speaks {PROTOCOL}"),
                    );
                    return false;
                }
                host.ok(id, Value::Null);
            }
            Request::Quit => {
                host.ok(id, Value::Null);
                return false;
            }
            Request::List => on_worker(host, id, |h| {
                h.simctl
                    .list()
                    .map(|l| json!({ "devices": l }))
                    .map_err(|e| (Fail::Unavailable, e.message))
            }),
            Request::Boot { target } => on_worker(host, id, move |h| {
                h.log(&format!("{target}: boot"));
                h.simctl.boot(&target).map(|()| Value::Null).map_err(|e| {
                    (
                        if e.timed_out {
                            Fail::Busy
                        } else {
                            Fail::Unavailable
                        },
                        e.message,
                    )
                })
            }),
            Request::Shutdown { target } => on_worker(host, id, move |h| {
                h.log(&format!("{target}: shutdown"));
                // Its next boot picks its input path again.
                if let Ok(mut p) = h.paths.lock() {
                    p.forget(&target);
                }
                h.simctl
                    .shutdown(&target)
                    .map(|()| Value::Null)
                    .map_err(|e| (Fail::Unavailable, e.message))
            }),
            Request::Launch {
                target,
                bundle_id,
                args,
            } => on_worker(host, id, move |h| {
                h.simctl
                    .launch(&target, &bundle_id, &args)
                    .map(|()| Value::Null)
                    .map_err(|e| (Fail::Unavailable, e.message))
            }),
            Request::Install { target, path } => on_worker(host, id, move |h| {
                h.simctl
                    .install(&target, Path::new(&path))
                    .map(|()| Value::Null)
                    .map_err(|e| (Fail::Unavailable, e.message))
            }),
            Request::Attach { target } => attach(host, id, target),
            Request::Detach { target } => {
                let a = host
                    .attached
                    .lock()
                    .ok()
                    .and_then(|mut a| a.remove(&target));
                if let Ok(mut s) = host.streams.lock() {
                    s.remove(&target);
                }
                forget_device(host, &target);
                if let Some(a) = a {
                    a.detach();
                    host.log(&format!("{target}: detached"));
                }
                host.ok(id, Value::Null);
            }
            Request::Pause { target } | Request::Resume { target }
                if !host.streams.lock().is_ok_and(|s| s.contains_key(&target)) =>
            {
                host.err(id, Fail::Unavailable, format!("{target} is not attached"));
            }
            Request::Pause { target } => {
                if let Some(s) = host
                    .streams
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&target).cloned())
                {
                    s.pause(true);
                }
                host.ok(id, Value::Null);
            }
            Request::Resume { target } => {
                if let Some(s) = host
                    .streams
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&target).cloned())
                {
                    s.pause(false);
                }
                host.ok(id, Value::Null);
            }
            Request::Touch { target, touch } => {
                if let Some(i) = input_of(host, &target) {
                    i.send(Cmd::Touch(touch));
                }
            }
            Request::Key { target, key } => {
                if let Some(i) = input_of(host, &target) {
                    i.send(Cmd::Key(key));
                }
            }
            Request::Button { target, button } => match input_of(host, &target) {
                Some(i) => i.send(Cmd::Button { id, button }),
                None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            },
            Request::Text { target, text } => match input_of(host, &target) {
                Some(i) => i.send(Cmd::Text { id, text }),
                None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
            },
            Request::Snapshot { target } => {
                let stream = host
                    .streams
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&target).cloned());
                let o = host
                    .orientations
                    .lock()
                    .ok()
                    .and_then(|o| {
                        o.get(&target)
                            .map(|o| *o.lock().unwrap_or_else(|p| p.into_inner()))
                    })
                    .unwrap_or_default();
                match stream {
                    None => host.err(id, Fail::Unavailable, format!("{target} is not attached")),
                    Some(s) => on_worker(host, id, move |h| {
                        let started = Instant::now();
                        let fb = s
                            .framebuffer()
                            .ok_or((Fail::Unavailable, "no framebuffer".to_string()))?;
                        let png = snapshot::png(&h.gpu, &fb, s.scale(), o)
                            .map_err(|e| (Fail::Unavailable, e))?;
                        h.log(&format!(
                            "{target}: snapshot {} KB in {} ms",
                            png.len() / 1024,
                            started.elapsed().as_millis()
                        ));
                        Ok(png_result(&png))
                    }),
                }
            }
            Request::UiTree { target, at } => {
                let attached = host.streams.lock().is_ok_and(|s| s.contains_key(&target));
                match &host.ax {
                    _ if !attached => {
                        host.err(id, Fail::Unavailable, format!("{target} is not attached"))
                    }
                    None => host.err(id, Fail::Unavailable, "this Xcode has no UI tree"),
                    Some(a) => {
                        let job = AxJob {
                            token: target.id().to_string(),
                            at,
                            reply: Some((id, Arc::downgrade(host))),
                        };
                        if !a.submit(job) {
                            host.err(id, Fail::Busy, "the UI tree is busy");
                        }
                    }
                }
            }
        }
        true
    }

    fn attach(host: &Arc<Host>, id: u64, target: Target) {
        if host.streams.lock().is_ok_and(|s| s.contains_key(&target)) {
            // Attached already (a second panel of one device): Flare holds its current ring, nothing is resent.
            host.ok(id, Value::Null);
            return;
        }
        let started = Instant::now();
        let device = match host.sim.device(&target) {
            Ok(d) => Arc::new(d),
            Err(b) => {
                host.log(&format!("{target}: attach failed: {b}"));
                host.err(id, Fail::Unavailable, b.to_string());
                return;
            }
        };
        // The interface's orientation before the first ring, so its screen facts carry it.
        let o = match orient::read(device.ptr(), target.id()) {
            Ok(o) => o,
            Err(b) => {
                host.log(&format!(
                    "{target}: orientation unread, portrait assumed: {b}"
                ));
                Orientation::Rot0
            }
        };
        let orientation = Arc::new(Mutex::new(o));
        let weak = Arc::downgrade(host);
        let props_target = target.clone();
        let r = sim::attach(
            &host.sim,
            &target,
            AttachOptions {
                sink: Arc::new(HostSink(Arc::downgrade(host))),
                gpu: Arc::clone(&host.gpu),
                ids: Arc::clone(&host.ids),
                probe_barcode: host.probe_barcode,
                orientation: Arc::clone(&orientation),
                // The properties change about 270 ms after a rotation: read the orientation again (in order with
                // the input, on its worker).
                on_props: Some(Arc::new(move || {
                    if let Some(h) = weak.upgrade()
                        && let Some(i) = input_of(&h, &props_target)
                    {
                        i.send(Cmd::ReadOrientation);
                    }
                })),
            },
        );
        match r {
            Ok(a) => {
                let portrait = a
                    .stream
                    .portrait_pt()
                    .unwrap_or(flare_device::geom::Size::new(0.0, 0.0));
                let path = host
                    .paths
                    .lock()
                    .map(|mut p| p.pick(&target, &host.coresim_version))
                    .unwrap_or(flare_device::versions::InputPath::Dtuhid);
                let input = Input::start(input::Setup {
                    target: target.clone(),
                    device: Arc::clone(&device),
                    path,
                    portrait,
                    orientation: Arc::clone(&orientation),
                    simctl: host.simctl.clone(),
                    restarted: host.restarted,
                    out: Arc::new(HostOut(Arc::downgrade(host))),
                });
                if let Ok(mut i) = host.inputs.lock() {
                    i.insert(target.clone(), Arc::new(input));
                }
                if let Ok(mut m) = host.orientations.lock() {
                    m.insert(target.clone(), orientation);
                }
                // The UI tree: the device's token, and one request now (the first after a boot takes about 3 s).
                ax::register(target.id(), &device);
                if let Some(a) = &host.ax {
                    a.submit(AxJob {
                        token: target.id().to_string(),
                        at: None,
                        reply: None,
                    });
                }
                if let Ok(mut s) = host.streams.lock() {
                    s.insert(target.clone(), Arc::clone(&a.stream));
                }
                if let Ok(mut m) = host.attached.lock() {
                    m.insert(target.clone(), a);
                }
                host.log(&format!(
                    "{target}: attached in {} ms ({path:?} input, {o:?})",
                    started.elapsed().as_millis()
                ));
                host.ok(id, Value::Null);
            }
            Err(b) => {
                host.log(&format!("{target}: attach failed: {b}"));
                host.err(id, Fail::Unavailable, b.to_string());
            }
        }
    }
}
