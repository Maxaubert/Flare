//! `flare-sim-host --bench <udid|new> <seconds>`: the frame path against the probe app (a port of the 2026-11-17
//! `zc.m` probe), with a fake consumer in place of Flare that frees each slot late (17 ms by default, one frame late
//! as Flare's `on_submitted_work_done` is). Prints JSON: distinct and skipped app frames (read from the probe app's
//! barcode in each slot sent), the interval between frames sent (p50, p99), GPU ms per slot, "no free slot" drops,
//! frames capped, and the sim host's CPU (`getrusage`). `new` makes a device for the run and deletes it after; it
//! never touches the person's devices. Headless: nothing opens a window.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::ring::{Gpu, GpuRing};
use super::sim::{self, AttachOptions, Sim, Sink, Stream, barcode};
use super::simctl::Simctl;
use crate::source::Screen;
use crate::target::Target;
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

#[cfg(test)]
mod tests {
    use super::*;

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
