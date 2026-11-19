//! flare-sim-host: Flare's helper for the device panel (issue #248). The only process that loads CoreSimulator and
//! SimulatorKit (rule 1310); one per Flare, started guarded so it dies with Flare (rule 1312).
//!
//! Modes:
//! - `--rendezvous <service> [--log <file>] [--probe-barcode]`: serve Flare. Control comes as JSON lines on stdin
//!   (`flare_device::wire::Control`), answers and events go as JSON lines on stdout (`HostLine`, a `hello` first).
//!   Frames go over the mach channel the service name sets up. The log is `<profile>/logs/sim-host.log` (1 MB x 3).
//!   PR 2 serves the frames, the device list and `simctl`; input, the UI tree and snapshots answer `unavailable` until
//!   PR 3.
//! - `--self-test`: load the frameworks and check every class and selector; JSON out, exit 0 when they are all there
//!   (or when there is no Xcode to test, with the reason).
//! - `--bench <udid|new> <seconds> [--heavy] [--probe-app <path>] [--late-ms <n>]`: the frame path against the probe
//!   app; JSON out.
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

    use flare_device::job::Fail;
    use flare_device::macos::bench::{self, BenchArgs};
    use flare_device::macos::load;
    use flare_device::macos::rendezvous::{Link, RecvError};
    use flare_device::macos::ring::{Gpu, GpuRing};
    use flare_device::macos::sim::{self, AttachOptions, Attached, Sim, Sink, Stream};
    use flare_device::macos::simctl::Simctl;
    use flare_device::source::Screen;
    use flare_device::target::Target;
    use flare_device::wire::{Control, HostLine, PROTOCOL, Record, Request, from_line, to_line};
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
        if let Some(name) = value("--rendezvous") {
            return serve(
                &name,
                value("--log").map(PathBuf::from),
                flag("--probe-barcode"),
            );
        }
        eprintln!(
            "usage: flare-sim-host --rendezvous <service> [--log <file>] [--probe-barcode] | --self-test | --bench <udid|new> <seconds>"
        );
        2
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
        attached: Mutex<HashMap<Target, Attached>>,
        streams: Mutex<HashMap<Target, Arc<Stream>>>,
        devices: Mutex<HashMap<u32, Target>>,
        records_dropped: AtomicU64,
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

    fn serve(name: &str, log_path: Option<PathBuf>, probe_barcode: bool) -> i32 {
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
            hello.input_path
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
        let _ = writeln!(out, "{}", to_line(&HostLine::Hello(hello.clone())));
        let _ = out.flush();
        let host = Arc::new(Host {
            out: Mutex::new(out),
            log: Mutex::new(log),
            link,
            sim,
            gpu,
            ids: Arc::new(AtomicU32::new(1)),
            simctl: Simctl::new(&dev),
            probe_barcode,
            attached: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            records_dropped: AtomicU64::new(0),
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
        let all: Vec<Attached> = host
            .attached
            .lock()
            .map(|mut a| a.drain().map(|(_, v)| v).collect())
            .unwrap_or_default();
        for a in all {
            a.detach();
        }
        if let Ok(mut s) = host.streams.lock() {
            s.clear();
        }
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
            Request::Touch { .. }
            | Request::Key { .. }
            | Request::Button { .. }
            | Request::Text { .. }
            | Request::Snapshot { .. }
            | Request::UiTree { .. } => {
                host.err(
                    id,
                    Fail::Unavailable,
                    "not in this sim host yet (input, the UI tree and snapshots come next)",
                );
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
        let r = sim::attach(
            &host.sim,
            &target,
            AttachOptions {
                sink: Arc::new(HostSink(Arc::downgrade(host))),
                gpu: Arc::clone(&host.gpu),
                ids: Arc::clone(&host.ids),
                probe_barcode: host.probe_barcode,
            },
        );
        match r {
            Ok(a) => {
                if let Ok(mut s) = host.streams.lock() {
                    s.insert(target.clone(), Arc::clone(&a.stream));
                }
                if let Ok(mut m) = host.attached.lock() {
                    m.insert(target.clone(), a);
                }
                host.log(&format!(
                    "{target}: attached in {} ms",
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
