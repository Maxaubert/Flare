//! `flare-sim-host --bench <udid|new> <seconds>`: the frame path against the probe app (a port of the 2026-11-17
//! `zc.m` probe), with a fake consumer in place of Flare that frees each slot late (17 ms by default, one frame late
//! as Flare's `on_submitted_work_done` is). Prints JSON: distinct and skipped app frames (read from the probe app's
//! barcode in each slot sent), the interval between frames sent (p50, p99), GPU ms per slot, "no free slot" drops,
//! frames capped, and the sim host's CPU (`getrusage`). `new` makes a device for the run and deletes it after; it
//! never touches the person's devices. Headless: nothing opens a window.
//!
//! `flare-sim-host --input-bench <udid|new>`: the input path against the touch probe app (a port of spike S1's
//! `hid.m`), through the same input worker Flare's panel uses. The app paints, as barcodes, its frame number and the
//! touches and keys it saw; the bench reads them from each frame and times press to painted frame. Taps, swipes,
//! typed characters, a pinch, then taps in landscape (rotated with the Purple event), which settles whether touch
//! points must be turned back to the portrait digitizer (`geom::to_portrait_ratio`). Prints JSON; exit 0 when the
//! gate passes (tap p50 at most 50 ms, every tap within 1 pt, every character).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::input::{self, Cmd, Input};
use super::orient;
use super::ring::{Gpu, GpuRing, read_pixels};
use super::sim::{self, AttachOptions, Sim, Sink, Stream, barcode};
use super::simctl::Simctl;
use crate::geom::{Orientation, Size, to_portrait_ratio};
use crate::job::Fail;
use crate::source::{Button, InputState, Phase, Screen, Touch};
use crate::target::Target;
use crate::versions::{self, InputPath};
use crate::wire::Record;

/// The probe app's bundle id (`tools/device-probe/Info.plist`).
pub const PROBE_BUNDLE: &str = "com.flare.test.FPSProbe";
/// Where `tools/device-probe/build.sh` puts the app, from the repository's root.
pub const PROBE_APP: &str = "tools/device-probe/build/FPSProbe.app";
/// The name of a device the bench makes.
pub const BENCH_DEVICE: &str = "Flare Bench iPhone";

/// What the bench was asked.
#[derive(Clone, Debug)]
pub struct BenchArgs {
    /// A device's udid, or "new".
    pub device: String,
    pub seconds: f64,
    /// The probe app's `-heavy` mode (the whole screen changes every frame).
    pub heavy: bool,
    pub probe_app: Option<PathBuf>,
    /// How late the fake consumer frees each slot.
    pub late_ms: u64,
}

/// The bench's numbers.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BenchResult {
    pub device: String,
    pub made_device: bool,
    pub heavy: bool,
    pub late_ms: u64,
    pub seconds: f64,
    pub callbacks: u64,
    pub sent: u64,
    pub distinct: u64,
    pub skipped: u64,
    pub interval_ms_p50: f64,
    pub interval_ms_p99: f64,
    pub interval_ms_max: f64,
    pub gpu_ms_p50: f64,
    pub gpu_ms_p99: f64,
    pub no_free_slot: u64,
    pub capped: u64,
    pub fill_errors: u64,
    pub cpu_percent: f64,
    pub ring_slots: usize,
}

/// The `p` percentile of `v` (sorted here).
pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Distinct app frames and the frames skipped between them, from the barcodes of the slots sent in order.
pub fn count_frames(codes: &[u16]) -> (u64, u64) {
    let mut distinct = 0u64;
    let mut skipped = 0u64;
    let mut prev: Option<u16> = None;
    for &c in codes {
        if prev != Some(c) {
            distinct += 1;
            if let Some(p) = prev {
                let step = c.wrapping_sub(p);
                // A step back (the app restarted) is not a skip.
                if step > 1 && step < 0x8000 {
                    skipped += u64::from(step - 1);
                }
            }
        }
        prev = Some(c);
    }
    (distinct, skipped)
}

/// Process CPU time (user + system) in seconds.
fn cpu_seconds() -> f64 {
    // SAFETY: getrusage fills a zeroed struct we own.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
    t(ru.ru_utime) + t(ru.ru_stime)
}

/// The fake consumer's side: rings by number and the records to it.
struct BenchSink {
    rings: Mutex<HashMap<u32, Arc<GpuRing>>>,
    tx: Mutex<Sender<Record>>,
}

impl Sink for BenchSink {
    fn ring(
        &self,
        _target: &Target,
        device: u32,
        _ring: u32,
        _screen: Screen,
        gpu_ring: &Arc<GpuRing>,
    ) {
        if let Ok(mut r) = self.rings.lock() {
            r.insert(device, Arc::clone(gpu_ring));
        }
    }

    fn record(&self, record: Record) -> bool {
        self.tx.lock().is_ok_and(|tx| tx.send(record).is_ok())
    }

    fn log(&self, line: &str) {
        eprintln!("bench: {line}");
    }
}

/// The fake consumer: reads each slot's barcode, frees it `late` after it came. Returns (t_us, barcode) per frame.
fn consume(
    rx: Receiver<Record>,
    sink: Arc<BenchSink>,
    stream: Arc<Mutex<Option<Arc<Stream>>>>,
    late: Duration,
    scale: f64,
) -> Vec<(u64, Option<u16>)> {
    let mut seen = Vec::new();
    let mut due: VecDeque<(Instant, u32, u32)> = VecDeque::new();
    loop {
        let wait = due
            .front()
            .map(|(t, _, _)| t.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100));
        match rx.recv_timeout(wait) {
            // sync-ok: the bench's own consumer thread
            Ok(Record::Frame(f)) => {
                let ring = sink
                    .rings
                    .lock()
                    .ok()
                    .and_then(|r| r.get(&f.device).cloned());
                let code = ring
                    .as_ref()
                    .and_then(|r| r.surfaces.get(f.slot as usize))
                    .and_then(|s| barcode(s, scale));
                seen.push((f.t_us, code));
                due.push_back((Instant::now() + late, f.device, f.slot));
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while due.front().is_some_and(|(t, _, _)| *t <= Instant::now()) {
            let (_, device, slot) = due.pop_front().unwrap_or((Instant::now(), 0, 0));
            if let Some(s) = stream.lock().ok().and_then(|s| s.clone()) {
                s.slot_done(device, slot);
            }
        }
    }
    seen
}

/// Deletes the device the bench made, however the run ends.
struct Made<'a> {
    simctl: &'a Simctl,
    target: Option<Target>,
}

impl Drop for Made<'_> {
    fn drop(&mut self) {
        if let Some(t) = self.target.take() {
            let _ = self.simctl.shutdown(&t);
            let _ = self.simctl.delete(&t);
        }
    }
}

/// Runs the bench. BLOCKING (boots a device, runs for `seconds`).
pub fn run(args: &BenchArgs, developer_dir: &Path) -> Result<BenchResult, String> {
    let simctl = Simctl::new(developer_dir);
    let mut made = Made {
        simctl: &simctl,
        target: None,
    };
    let target = if args.device == "new" {
        let ty = simctl.iphone_type().map_err(|e| e.to_string())?;
        let t = simctl
            .create(BENCH_DEVICE, &ty)
            .map_err(|e| e.to_string())?;
        made.target = Some(t.clone());
        t
    } else {
        Target::sim(&args.device).ok_or_else(|| format!("not a udid: {}", args.device))?
    };
    eprintln!("bench: booting {target} (headless)");
    simctl.boot(&target).map_err(|e| format!("boot: {e}"))?;
    let app = args
        .probe_app
        .clone()
        .unwrap_or_else(|| PathBuf::from(PROBE_APP));
    if app.exists() {
        simctl
            .install(&target, &app)
            .map_err(|e| format!("install: {e}"))?;
        let launch_args: Vec<String> = if args.heavy {
            vec!["-heavy".into()]
        } else {
            Vec::new()
        };
        simctl
            .launch(&target, PROBE_BUNDLE, &launch_args)
            .map_err(|e| format!("launch: {e}"))?;
        std::thread::sleep(Duration::from_secs(3)); // sync-ok: the bench lets the app draw first
    } else if made.target.is_some() {
        return Err(format!(
            "no probe app at {} (build it with tools/device-probe/build.sh)",
            app.display()
        ));
    }
    let sim = Sim::open(developer_dir).map_err(|e| e.to_string())?;
    let gpu = Arc::new(Gpu::new()?);
    let (tx, rx) = channel();
    let sink = Arc::new(BenchSink {
        rings: Mutex::new(HashMap::new()),
        tx: Mutex::new(tx),
    });
    let slot_stream: Arc<Mutex<Option<Arc<Stream>>>> = Arc::new(Mutex::new(None));
    let (_, scale) = sim
        .device(&target)
        .and_then(|d| d.screen_facts())
        .map_err(|e| e.to_string())?;
    let consumer = {
        let sink = Arc::clone(&sink);
        let s = Arc::clone(&slot_stream);
        let late = Duration::from_millis(args.late_ms);
        std::thread::spawn(move || consume(rx, sink, s, late, scale))
    };
    let cpu0 = cpu_seconds();
    let t0 = Instant::now();
    let attached = sim::attach(
        &sim,
        &target,
        AttachOptions {
            sink: Arc::clone(&sink) as Arc<dyn Sink>,
            gpu,
            ids: Arc::new(AtomicU32::new(1)),
            probe_barcode: false,
            orientation: Arc::default(),
            on_props: None,
        },
    )
    .map_err(|e| e.to_string())?;
    if let Ok(mut s) = slot_stream.lock() {
        *s = Some(Arc::clone(&attached.stream));
    }
    std::thread::sleep(Duration::from_secs_f64(args.seconds)); // sync-ok: the bench's run
    let stats = attached.stream.stats();
    let mut gpu_ms = attached.stream.gpu_ms();
    let wall = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu0;
    attached.detach();
    if let Ok(mut s) = slot_stream.lock() {
        *s = None;
    }
    if let Ok(mut tx) = sink.tx.lock() {
        // Ends the consumer: its channel's only sender goes.
        *tx = channel().0;
    }
    let seen = consumer.join().unwrap_or_default(); // sync-ok: the consumer ends with its channel
    let codes: Vec<u16> = seen.iter().filter_map(|(_, c)| *c).collect();
    let (distinct, skipped) = count_frames(&codes);
    let mut intervals: Vec<f64> = seen
        .windows(2)
        .map(|w| w[1].0.saturating_sub(w[0].0) as f64 / 1000.0)
        .collect();
    let max = intervals.iter().copied().fold(0.0, f64::max);
    Ok(BenchResult {
        device: if made.target.is_some() {
            "new".into()
        } else {
            target.name()
        },
        made_device: made.target.is_some(),
        heavy: args.heavy,
        late_ms: args.late_ms,
        seconds: args.seconds,
        callbacks: stats.callbacks,
        sent: stats.sent,
        distinct,
        skipped,
        interval_ms_p50: percentile(&mut intervals, 0.5),
        interval_ms_p99: percentile(&mut intervals, 0.99),
        interval_ms_max: max,
        gpu_ms_p50: percentile(&mut gpu_ms, 0.5),
        gpu_ms_p99: percentile(&mut gpu_ms, 0.99),
        no_free_slot: stats.dropped,
        capped: stats.capped,
        fill_errors: stats.fill_errors,
        cpu_percent: if wall > 0.0 { cpu / wall * 100.0 } else { 0.0 },
        ring_slots: crate::cap::RING_SLOTS,
    })
}

/// The touch probe app (`tools/device-probe/TouchProbe-Info.plist`) and where `build.sh` puts it.
pub const TOUCH_PROBE_BUNDLE: &str = "com.flare.test.TouchProbe";
pub const TOUCH_PROBE_APP: &str = "tools/device-probe/build/TouchProbe.app";
/// The touch probe's barcode rows: 9 rows of 16 blocks, 24 pt tall, from 120 pt down, in a band as wide as the
/// screen's short side, centred.
const ROWS: usize = 9;
/// Row meanings.
const R_BEGAN: usize = 1;
const R_X: usize = 2;
const R_Y: usize = 3;
const R_ENDED: usize = 5;
const R_CHARS: usize = 6;
const R_MOST: usize = 7;
const R_MOVED: usize = 8;

/// What the input bench was asked.
#[derive(Clone, Debug)]
pub struct InputBenchArgs {
    pub device: String,
    pub probe_app: Option<PathBuf>,
    pub taps: usize,
    pub swipes: usize,
    pub chars: usize,
    pub landscape_taps: usize,
}

/// The input bench's numbers.
#[derive(Clone, Debug, Default, Serialize)]
pub struct InputBenchResult {
    pub device: String,
    pub made_device: bool,
    pub coresim_version: String,
    pub input_path: String,
    pub boot_s: f64,
    /// From the input worker's start to a live connection (liveness tries and the 200 ms settle included).
    pub connect_ms: f64,
    pub taps: usize,
    pub taps_seen: usize,
    pub taps_landed: usize,
    pub tap_ms_p50: f64,
    pub tap_ms_p90: f64,
    pub tap_ms_p99: f64,
    pub tap_ms_max: f64,
    pub tap_worst_error_pt: f64,
    pub swipes: usize,
    pub swipes_began: usize,
    pub swipes_ended: usize,
    pub swipe_moves_sent: usize,
    pub swipe_moves_seen: u64,
    pub swipe_start_ms_p50: f64,
    pub swipe_start_ms_max: f64,
    pub chars: usize,
    pub chars_arrived: usize,
    pub key_ms_p50: f64,
    pub key_ms_p99: f64,
    pub key_ms_max: f64,
    pub pinch_most_touches: u16,
    /// The interface's orientations reached by turning right, then left twice ("rot90,rot270" when the app turned).
    pub landscape: String,
    pub landscape_taps: usize,
    pub landscape_landed: usize,
    pub landscape_worst_error_pt: f64,
    pub passed: bool,
    pub notes: Vec<String>,
}

/// One frame's barcodes and when its callback came (the sim host's clock, `sim::now_us`).
#[derive(Clone, Copy, Debug)]
struct Probe {
    t_us: u64,
    rows: [u16; ROWS],
}

/// The pixels of the probe's barcode blocks in a portrait surface of `px` pixels, for the interface turned `o`
/// (the app draws in interface points; the framebuffer stays portrait).
fn probe_points(o: Orientation, portrait: Size, px: (usize, usize)) -> Vec<(usize, usize)> {
    let shown = o.size(portrait);
    // The app draws the band as wide as the short side, centred (clear of the Dynamic Island in landscape).
    let band = shown.w.min(shown.h);
    let x0 = (shown.w - band) / 2.0;
    let mut out = Vec::with_capacity(ROWS * 16);
    for r in 0..ROWS {
        for i in 0..16 {
            let ix = x0 + (i as f64 + 0.5) * band / 16.0;
            let iy = 120.0 + 24.0 * r as f64 + 12.0;
            let (rx, ry) = to_portrait_ratio(ix, iy, portrait, o);
            let x = ((rx * px.0 as f64) as usize).min(px.0.saturating_sub(1));
            let y = ((ry * px.1 as f64) as usize).min(px.1.saturating_sub(1));
            out.push((x, y));
        }
    }
    out
}

/// The 9 barcodes of a frame's pixels (a block is lit when its colour sum is over 384).
fn decode_rows(px: &[[u8; 4]]) -> [u16; ROWS] {
    let mut rows = [0u16; ROWS];
    for (r, row) in rows.iter_mut().enumerate() {
        for i in 0..16 {
            let p = px[r * 16 + i];
            let lit = u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 384;
            *row = (*row << 1) | u16::from(lit);
        }
    }
    rows
}

/// The frames seen so far.
#[derive(Default)]
struct Frames {
    list: Mutex<Vec<Probe>>,
}

impl Frames {
    fn len(&self) -> usize {
        self.list.lock().map_or(0, |l| l.len())
    }

    fn latest(&self) -> Option<Probe> {
        self.list.lock().ok().and_then(|l| l.last().copied())
    }

    /// The first frame from index `from` whose `row` reads `want`, within `limit`.
    fn wait_row(&self, from: usize, row: usize, want: u16, limit: Duration) -> Option<Probe> {
        let end = Instant::now() + limit;
        let mut at = from;
        while Instant::now() < end {
            if let Ok(l) = self.list.lock() {
                if let Some(p) = l.iter().skip(at).find(|p| p.rows[row] == want) {
                    return Some(*p);
                }
                at = l.len();
            }
            std::thread::sleep(Duration::from_micros(300)); // sync-ok: the bench's own wait
        }
        None
    }
}

/// The input bench's sink: each frame's slot is read for the barcodes, then freed.
struct ProbeSink {
    rings: Mutex<HashMap<u32, Arc<GpuRing>>>,
    tx: Mutex<Sender<Record>>,
}

impl Sink for ProbeSink {
    fn ring(&self, _: &Target, device: u32, _: u32, _: Screen, gpu_ring: &Arc<GpuRing>) {
        if let Ok(mut r) = self.rings.lock() {
            r.insert(device, Arc::clone(gpu_ring));
        }
    }

    fn record(&self, record: Record) -> bool {
        self.tx.lock().is_ok_and(|tx| tx.send(record).is_ok())
    }

    fn log(&self, line: &str) {
        eprintln!("input bench: {line}");
    }
}

/// An input command's answer.
type Answered = Result<(), (Fail, String)>;

/// The input worker's answers and news, kept for the bench.
#[derive(Default)]
struct BenchOut {
    states: Mutex<Vec<(Instant, InputState)>>,
    answers: Mutex<HashMap<u64, Answered>>,
}

impl input::Out for BenchOut {
    fn answer(&self, id: u64, r: Result<(), (Fail, String)>) {
        if let Ok(mut a) = self.answers.lock() {
            a.insert(id, r);
        }
    }
    fn input(&self, _: &Target, state: InputState) {
        eprintln!("input bench: input {state:?}");
        if let Ok(mut s) = self.states.lock() {
            s.push((Instant::now(), state));
        }
    }
    fn orientation(&self, _: &Target, o: Orientation) {
        eprintln!("input bench: interface {o:?}");
    }
    fn log(&self, line: &str) {
        eprintln!("input bench: {line}");
    }
}

impl BenchOut {
    fn wait_answer(&self, id: u64, limit: Duration) -> Option<Answered> {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if let Some(r) = self.answers.lock().ok().and_then(|mut a| a.remove(&id)) {
                return Some(r);
            }
            std::thread::sleep(Duration::from_millis(2)); // sync-ok: the bench's own wait
        }
        None
    }
}

/// A small seeded random source (xorshift), so a run can be repeated.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
}

/// Runs the input bench. BLOCKING (boots a device, a minute or two).
pub fn run_input(
    args: &InputBenchArgs,
    developer_dir: &Path,
    coresim_version: &str,
) -> Result<InputBenchResult, String> {
    let simctl = Simctl::new(developer_dir);
    let mut made = Made {
        simctl: &simctl,
        target: None,
    };
    let target = if args.device == "new" {
        let ty = simctl.iphone_type().map_err(|e| e.to_string())?;
        let t = simctl
            .create(BENCH_DEVICE, &ty)
            .map_err(|e| e.to_string())?;
        made.target = Some(t.clone());
        t
    } else {
        Target::sim(&args.device).ok_or_else(|| format!("not a udid: {}", args.device))?
    };
    let app = args
        .probe_app
        .clone()
        .unwrap_or_else(|| PathBuf::from(TOUCH_PROBE_APP));
    if !app.exists() {
        return Err(format!(
            "no touch probe app at {} (build it with tools/device-probe/build.sh)",
            app.display()
        ));
    }
    let mut res = InputBenchResult {
        device: if made.target.is_some() {
            "new".into()
        } else {
            target.name()
        },
        made_device: made.target.is_some(),
        coresim_version: coresim_version.to_string(),
        ..Default::default()
    };
    eprintln!("input bench: booting {target} (headless)");
    let t_boot = Instant::now();
    simctl.boot(&target).map_err(|e| format!("boot: {e}"))?;
    res.boot_s = t_boot.elapsed().as_secs_f64();
    let sim = Sim::open(developer_dir).map_err(|e| e.to_string())?;
    let device = Arc::new(sim.device(&target).map_err(|e| e.to_string())?);
    let (px, scale) = device.screen_facts().map_err(|e| e.to_string())?;
    let portrait = Size::new(px.w / scale, px.h / scale);
    let orientation = Arc::new(Mutex::new(
        orient::read(device.ptr(), target.udid()).unwrap_or(Orientation::Rot0),
    ));
    // The input worker, as Flare's panel runs it; the cold connect right after the boot.
    let path = versions::input_path(coresim_version);
    res.input_path = format!("{path:?}");
    let out = Arc::new(BenchOut::default());
    let t_connect = Instant::now();
    let input = Input::start(input::Setup {
        target: target.clone(),
        device: Arc::clone(&device),
        path,
        portrait,
        orientation: Arc::clone(&orientation),
        simctl: simctl.clone(),
        restarted: false,
        out: Arc::clone(&out) as Arc<dyn input::Out>,
    });
    let live = loop {
        let last = out.states.lock().ok().and_then(|s| s.last().cloned());
        match last {
            Some((t, InputState::Live)) => break t,
            Some((_, InputState::Broken { why })) => return Err(format!("no input: {why}")),
            _ if t_connect.elapsed() > Duration::from_secs(60) => {
                return Err("no input in 60 s".into());
            }
            _ => std::thread::sleep(Duration::from_millis(5)), // sync-ok: the bench's own wait
        }
    };
    res.connect_ms = live.duration_since(t_connect).as_secs_f64() * 1000.0;
    simctl
        .install(&target, &app)
        .map_err(|e| format!("install: {e}"))?;
    simctl
        .launch(&target, TOUCH_PROBE_BUNDLE, &[])
        .map_err(|e| format!("launch: {e}"))?;
    std::thread::sleep(Duration::from_secs(3)); // sync-ok: the app draws first
    // The frames: each slot's barcodes, read at the probe's blocks for the current orientation.
    let (tx, rx) = channel::<Record>();
    let sink = Arc::new(ProbeSink {
        rings: Mutex::new(HashMap::new()),
        tx: Mutex::new(tx),
    });
    let frames = Arc::new(Frames::default());
    let stream_slot: Arc<Mutex<Option<Arc<Stream>>>> = Arc::new(Mutex::new(None));
    let consumer = {
        let (sink, frames, slot, o) = (
            Arc::clone(&sink),
            Arc::clone(&frames),
            Arc::clone(&stream_slot),
            Arc::clone(&orientation),
        );
        std::thread::spawn(move || {
            for r in rx {
                let Record::Frame(f) = r else {
                    continue;
                };
                let ring = sink
                    .rings
                    .lock()
                    .ok()
                    .and_then(|r| r.get(&f.device).cloned());
                if let Some(s) = ring.as_ref().and_then(|r| r.surfaces.get(f.slot as usize)) {
                    let or = *o.lock().unwrap_or_else(|p| p.into_inner());
                    let pts = probe_points(or, portrait, (s.width(), s.height()));
                    if let Some(px) = read_pixels(s, &pts)
                        && let Ok(mut l) = frames.list.lock()
                    {
                        l.push(Probe {
                            t_us: f.t_us,
                            rows: decode_rows(&px),
                        });
                    }
                }
                if let Some(s) = slot.lock().ok().and_then(|s| s.clone()) {
                    s.slot_done(f.device, f.slot);
                }
            }
        })
    };
    let attached = sim::attach(
        &sim,
        &target,
        AttachOptions {
            sink: Arc::clone(&sink) as Arc<dyn Sink>,
            gpu: Arc::new(Gpu::new()?),
            ids: Arc::new(AtomicU32::new(1)),
            probe_barcode: false,
            orientation: Arc::clone(&orientation),
            on_props: None,
        },
    )
    .map_err(|e| e.to_string())?;
    if let Ok(mut s) = stream_slot.lock() {
        *s = Some(Arc::clone(&attached.stream));
    }
    std::thread::sleep(Duration::from_millis(800)); // sync-ok: the first frames
    if frames.len() < 5 {
        return Err(format!("only {} frames from the probe app", frames.len()));
    }
    let mut rng = Rng(0x1119_2026_0000_0001);
    let touch = |phase, x, y| input.send(Cmd::Touch(Touch::one(phase, x, y)));
    let mut next_id = 1u64;

    // Taps.
    let mut tap_ms = Vec::new();
    let mut worst = 0.0f64;
    for _ in 0..args.taps {
        let (x, y) = (
            rng.range(20.0, portrait.w - 20.0),
            rng.range(80.0, portrait.h - 60.0),
        );
        let before = frames.latest().map(|p| p.rows).unwrap_or_default();
        let from = frames.len();
        let t0 = sim::now_us();
        touch(Phase::Start, x, y);
        let got = frames.wait_row(
            from,
            R_BEGAN,
            before[R_BEGAN].wrapping_add(1),
            Duration::from_millis(1500),
        );
        touch(Phase::End, x, y);
        if let Some(p) = got {
            res.taps_seen += 1;
            tap_ms.push(p.t_us.saturating_sub(t0) as f64 / 1000.0);
            let err = (f64::from(p.rows[R_X]) / 50.0 - x)
                .abs()
                .max((f64::from(p.rows[R_Y]) / 50.0 - y).abs());
            worst = worst.max(err);
            if err <= 1.0 {
                res.taps_landed += 1;
            }
        }
        frames.wait_row(
            from,
            R_ENDED,
            before[R_ENDED].wrapping_add(1),
            Duration::from_millis(1000),
        );
        std::thread::sleep(Duration::from_millis(60 + rng.next() % 80)); // sync-ok: the bench's pacing
    }
    res.taps = args.taps;
    res.tap_worst_error_pt = worst;
    res.tap_ms_max = tap_ms.iter().copied().fold(0.0, f64::max);
    res.tap_ms_p50 = percentile(&mut tap_ms, 0.5);
    res.tap_ms_p90 = percentile(&mut tap_ms, 0.9);
    res.tap_ms_p99 = percentile(&mut tap_ms, 0.99);

    // Swipes: a point every 8 pt, one every 16 ms.
    let mut swipe_ms = Vec::new();
    let moved0 = frames.latest().map_or(0, |p| p.rows[R_MOVED]);
    for _ in 0..args.swipes {
        let (x0, y0) = (
            rng.range(40.0, portrait.w - 40.0),
            rng.range(160.0, portrait.h - 160.0),
        );
        let ang = rng.range(0.0, std::f64::consts::TAU);
        let len = rng.range(100.0, 250.0);
        let x1 = (x0 + ang.cos() * len).clamp(30.0, portrait.w - 30.0);
        let y1 = (y0 + ang.sin() * len).clamp(140.0, portrait.h - 140.0);
        let before = frames.latest().map(|p| p.rows).unwrap_or_default();
        let from = frames.len();
        let t0 = sim::now_us();
        let path = crate::geom::swipe_path((x0, y0), (x1, y1), 0);
        touch(Phase::Start, x0, y0);
        for &(x, y, _) in path.iter().skip(1) {
            std::thread::sleep(Duration::from_millis(16)); // sync-ok: the swipe's pace
            touch(Phase::Move, x, y);
            res.swipe_moves_sent += 1;
        }
        touch(Phase::End, x1, y1);
        if let Some(p) = frames.wait_row(
            from,
            R_BEGAN,
            before[R_BEGAN].wrapping_add(1),
            Duration::from_millis(1500),
        ) {
            res.swipes_began += 1;
            swipe_ms.push(p.t_us.saturating_sub(t0) as f64 / 1000.0);
        }
        if frames
            .wait_row(
                from,
                R_ENDED,
                before[R_ENDED].wrapping_add(1),
                Duration::from_millis(1500),
            )
            .is_some()
        {
            res.swipes_ended += 1;
        }
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: the bench's pacing
    }
    res.swipes = args.swipes;
    res.swipe_moves_seen = u64::from(
        frames
            .latest()
            .map_or(0, |p| p.rows[R_MOVED])
            .wrapping_sub(moved0),
    );
    res.swipe_start_ms_max = swipe_ms.iter().copied().fold(0.0, f64::max);
    res.swipe_start_ms_p50 = percentile(&mut swipe_ms, 0.5);

    // Typing: one character at a time through the keymap (shift included).
    const ALPHA: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -=[];',./!@#$%^&*()_+{}:\"<>?";
    let alpha: Vec<char> = ALPHA.chars().collect();
    let mut key_ms = Vec::new();
    for _ in 0..args.chars {
        let ch = alpha[(rng.next() % alpha.len() as u64) as usize];
        let before = frames.latest().map(|p| p.rows).unwrap_or_default();
        let from = frames.len();
        let t0 = sim::now_us();
        let id = next_id;
        next_id += 1;
        input.send(Cmd::Text {
            id,
            text: ch.to_string(),
        });
        if let Some(p) = frames.wait_row(
            from,
            R_CHARS,
            before[R_CHARS].wrapping_add(1),
            Duration::from_millis(1000),
        ) {
            res.chars_arrived += 1;
            key_ms.push(p.t_us.saturating_sub(t0) as f64 / 1000.0);
        }
        let _ = out.wait_answer(id, Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(30)); // sync-ok: the bench's pacing
    }
    res.chars = args.chars;
    res.key_ms_max = key_ms.iter().copied().fold(0.0, f64::max);
    res.key_ms_p50 = percentile(&mut key_ms, 0.5);
    res.key_ms_p99 = percentile(&mut key_ms, 0.99);

    // One two-finger pinch.
    {
        let (cx, cy) = (portrait.w / 2.0, portrait.h / 2.0);
        let pinch = |phase, d: f64| {
            input.send(Cmd::Touch(Touch {
                phase,
                x: cx - d,
                y: cy - d,
                second: Some((cx + d, cy + d)),
            }))
        };
        pinch(Phase::Start, 30.0);
        for i in 1..=15 {
            std::thread::sleep(Duration::from_millis(16)); // sync-ok: the pinch's pace
            pinch(Phase::Move, 30.0 + f64::from(i) * 6.0);
        }
        pinch(Phase::End, 120.0);
        std::thread::sleep(Duration::from_millis(400)); // sync-ok: the app paints
        res.pinch_most_touches = frames.latest().map_or(0, |p| p.rows[R_MOST]);
    }

    // Landscape both ways: turn right (rot90), taps; turn left twice (rot270), taps; back to portrait. The taps are
    // in interface points of the turned screen, as Flare sends them.
    let mut worst = 0.0f64;
    let mut reached = Vec::new();
    for (turns, left) in [(1usize, false), (2, true)] {
        for _ in 0..turns {
            let id = next_id;
            next_id += 1;
            input.send(Cmd::Button {
                id,
                button: if left {
                    Button::RotateLeft
                } else {
                    Button::RotateRight
                },
            });
            match out.wait_answer(id, Duration::from_secs(5)) {
                Some(Ok(())) => {}
                other => res.notes.push(format!("rotate: {other:?}")),
            }
        }
        std::thread::sleep(Duration::from_millis(1000)); // sync-ok: the app lays out again
        let o = *orientation.lock().unwrap_or_else(|p| p.into_inner());
        reached.push(format!("{o:?}").to_lowercase());
        if o == Orientation::Rot0 {
            res.notes
                .push("the probe app did not turn: landscape taps skipped".into());
            continue;
        }
        let shown = o.size(portrait);
        for _ in 0..args.landscape_taps {
            let (x, y) = (
                rng.range(20.0, shown.w - 20.0),
                rng.range(40.0, shown.h - 30.0),
            );
            let before = frames.latest().map(|p| p.rows).unwrap_or_default();
            let from = frames.len();
            touch(Phase::Start, x, y);
            let got = frames.wait_row(
                from,
                R_BEGAN,
                before[R_BEGAN].wrapping_add(1),
                Duration::from_millis(1500),
            );
            touch(Phase::End, x, y);
            res.landscape_taps += 1;
            if let Some(p) = got {
                let err = (f64::from(p.rows[R_X]) / 50.0 - x)
                    .abs()
                    .max((f64::from(p.rows[R_Y]) / 50.0 - y).abs());
                worst = worst.max(err);
                if err <= 1.0 {
                    res.landscape_landed += 1;
                } else {
                    res.notes.push(format!(
                        "{o:?} tap at {x:.1},{y:.1} landed at {:.1},{:.1}",
                        f64::from(p.rows[R_X]) / 50.0,
                        f64::from(p.rows[R_Y]) / 50.0
                    ));
                }
            }
            frames.wait_row(
                from,
                R_ENDED,
                before[R_ENDED].wrapping_add(1),
                Duration::from_millis(1000),
            );
            std::thread::sleep(Duration::from_millis(80)); // sync-ok: the bench's pacing
        }
    }
    res.landscape = reached.join(",");
    res.landscape_worst_error_pt = worst;
    input.send(Cmd::Button {
        id: next_id,
        button: Button::RotateRight,
    });
    let _ = out.wait_answer(next_id, Duration::from_secs(5));
    res.passed = res.tap_ms_p50 <= 50.0
        && res.taps_landed == res.taps
        && res.chars_arrived == res.chars
        && res.landscape_landed == res.landscape_taps;
    if path == InputPath::Indigo {
        res.notes
            .push("legacy Indigo path (untested by the plan)".into());
    }
    drop(input);
    attached.detach();
    if let Ok(mut s) = stream_slot.lock() {
        *s = None;
    }
    if let Ok(mut tx) = sink.tx.lock() {
        *tx = channel().0;
    }
    let _ = consumer.join(); // sync-ok: the consumer ends with its channel
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_touch_probes_blocks_are_found_in_every_orientation() {
        let portrait = Size::new(402.0, 874.0);
        let px = (1206usize, 2622usize);
        // Portrait: row 0's first block is at (402/32, 132) pt.
        let p = probe_points(Orientation::Rot0, portrait, px);
        assert_eq!(p.len(), ROWS * 16);
        assert_eq!(p[0], ((402.0 / 32.0 * 3.0) as usize, 396));
        // Landscape (rot90): the shown top band is the portrait's left edge, so a row is a column of the buffer.
        let p = probe_points(Orientation::Rot90, portrait, px);
        assert!(
            p[..16].iter().all(|&(x, _)| x == p[0].0),
            "row 0 is one column: {:?}",
            &p[..16]
        );
        assert_eq!(p[0].0, 396);
        // Decoding: lit blocks are ones.
        let mut pix = vec![[0u8, 0, 0, 255]; ROWS * 16];
        pix[0] = [255, 255, 255, 255];
        pix[15] = [255, 255, 255, 255];
        pix[16 + 15] = [200, 200, 200, 255];
        let rows = decode_rows(&pix);
        assert_eq!(rows[0], 0b1000_0000_0000_0001);
        assert_eq!(rows[1], 1);
        assert_eq!(rows[2], 0);
    }

    #[test]
    fn frames_are_counted_from_the_barcodes() {
        assert_eq!(count_frames(&[]), (0, 0));
        assert_eq!(count_frames(&[5, 6, 7, 8]), (4, 0));
        assert_eq!(
            count_frames(&[5, 5, 6, 9]),
            (3, 2),
            "a repeat is one frame, 7 and 8 were skipped"
        );
        assert_eq!(
            count_frames(&[0xfffe, 0xffff, 0, 1]),
            (4, 0),
            "the 16-bit counter wraps"
        );
        assert_eq!(count_frames(&[100, 3]), (2, 0), "the app restarted");
        let mut v = vec![3.0, 1.0, 2.0, 100.0];
        assert_eq!(percentile(&mut v, 0.5), 3.0);
        assert_eq!(percentile(&mut v, 1.0), 100.0);
        assert_eq!(percentile(&mut [], 0.5), 0.0);
    }
}
