//! Frame-time statistics over a rolling window of samples.

use std::collections::VecDeque;

/// A rolling window of frame times in milliseconds. Pushing past `cap` drops the oldest sample.
#[derive(Debug, Clone)]
pub struct FrameStats {
    samples: VecDeque<f64>,
    cap: usize,
}

impl FrameStats {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push_ms(&mut self, ms: f64) {
        self.samples.push_back(ms);
        while self.samples.len() > self.cap {
            self.samples.pop_front();
        }
    }

    pub fn last(&self) -> Option<f64> {
        self.samples.back().copied()
    }

    pub fn avg(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        Some(self.samples.iter().sum::<f64>() / self.samples.len() as f64)
    }

    pub fn max(&self) -> Option<f64> {
        self.samples.iter().copied().reduce(f64::max)
    }

    /// Nearest-rank 95th percentile over a sorted copy of the window.
    pub fn p95(&self) -> Option<f64> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let idx = (95 * n).div_ceil(100) - 1;
        Some(sorted[idx])
    }

    /// How many samples the window holds.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Drops every sample.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Nearest-rank percentile (`p` in 0..=100) over a sorted copy of the window.
    pub fn percentile(&self, p: f64) -> Option<f64> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let mut sorted: Vec<f64> = self.samples.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let rank = ((p.clamp(0.0, 100.0) * n as f64) / 100.0 - 1e-9).ceil() as usize;
        Some(sorted[rank.clamp(1, n) - 1])
    }

    /// How many samples are above `ms`.
    pub fn over(&self, ms: f64) -> usize {
        self.samples.iter().filter(|s| **s > ms).count()
    }

    /// One line, for logs and the overlay: `last 1.20 ms, avg 1.10 ms, p95 1.90 ms, max 2.00 ms (n=20)`,
    /// or `no frames` when the window is empty.
    pub fn summary(&self) -> String {
        match (self.last(), self.avg(), self.p95(), self.max()) {
            (Some(last), Some(avg), Some(p95), Some(max)) => format!(
                "last {last:.2} ms, avg {avg:.2} ms, p95 {p95:.2} ms, max {max:.2} ms (n={})",
                self.samples.len()
            ),
            _ => "no frames".to_string(),
        }
    }
}

/// The parts of one frame the trace times (M10 Task 67), in the order they happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TracePhase {
    /// Waiting for the emulator lock.
    LockWait,
    /// Copying the visible rows out while holding it.
    Snapshot,
    /// Laying the grid out into rects and glyph draws.
    Scene,
    /// Rasterising new glyphs and uploading them to the atlas (a part of `Paint`).
    GlyphUpload,
    /// Painting the scene, the glyph upload included.
    Paint,
    /// Handing the image to Slint (the set-property call and what follows it).
    SetImage,
    /// The whole render callback, `BeforeRendering` to `AfterRendering`.
    Render,
    /// From `AfterRendering` to the next `BeforeRendering`: the present and the wait for the next frame.
    Gap,
}

/// How many phases there are.
pub const TRACE_PHASES: usize = 8;

impl TracePhase {
    /// Every phase, in order (the index of a phase is its position here).
    pub const ALL: [TracePhase; TRACE_PHASES] = [
        TracePhase::LockWait,
        TracePhase::Snapshot,
        TracePhase::Scene,
        TracePhase::GlyphUpload,
        TracePhase::Paint,
        TracePhase::SetImage,
        TracePhase::Render,
        TracePhase::Gap,
    ];

    /// The name the trace line uses.
    pub fn name(self) -> &'static str {
        match self {
            TracePhase::LockWait => "lock_wait",
            TracePhase::Snapshot => "snapshot",
            TracePhase::Scene => "scene",
            TracePhase::GlyphUpload => "glyph_upload",
            TracePhase::Paint => "paint",
            TracePhase::SetImage => "set_image",
            TracePhase::Render => "render",
            TracePhase::Gap => "gap",
        }
    }
}

/// Per-frame timings in microseconds, summed per phase until `take` (M10 Task 67, step 1). Off unless
/// `FLARE_TRACE_FRAME=1`; an off trace records nothing and costs one branch.
pub struct FrameTrace {
    on: bool,
    micros: [std::sync::atomic::AtomicU64; TRACE_PHASES],
    frames: std::sync::atomic::AtomicU64,
}

impl FrameTrace {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            micros: [const { std::sync::atomic::AtomicU64::new(0) }; TRACE_PHASES],
            frames: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Whether the variable's value turns the trace on: only "1".
    pub fn enabled_by(value: Option<&str>) -> bool {
        value == Some("1")
    }

    /// The process-wide trace, read from `FLARE_TRACE_FRAME` once.
    pub fn global() -> &'static FrameTrace {
        static TRACE: std::sync::OnceLock<FrameTrace> = std::sync::OnceLock::new();
        TRACE.get_or_init(|| {
            FrameTrace::new(Self::enabled_by(
                std::env::var("FLARE_TRACE_FRAME").ok().as_deref(),
            ))
        })
    }

    pub fn on(&self) -> bool {
        self.on
    }

    /// Adds `d` to the phase.
    pub fn add(&self, phase: TracePhase, d: std::time::Duration) {
        if !self.on {
            return;
        }
        let us = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.micros[phase as usize].fetch_add(us, std::sync::atomic::Ordering::Relaxed);
    }

    /// Times `f` into the phase (just runs it when the trace is off).
    pub fn time<R>(&self, phase: TracePhase, f: impl FnOnce() -> R) -> R {
        if !self.on {
            return f();
        }
        let start = std::time::Instant::now();
        let r = f();
        self.add(phase, start.elapsed());
        r
    }

    /// The sums since the last `take`, which are reset.
    pub fn take(&self) -> [u64; TRACE_PHASES] {
        let mut out = [0u64; TRACE_PHASES];
        for (o, m) in out.iter_mut().zip(&self.micros) {
            *o = m.swap(0, std::sync::atomic::Ordering::Relaxed);
        }
        out
    }

    /// Ends a frame: when on, logs the phase sums at `trace` level and resets them.
    pub fn end_frame(&self) {
        if !self.on {
            return;
        }
        let n = self
            .frames
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        log::trace!("{}", Self::format_line(n, &self.take()));
    }

    /// `frame 12 lock_wait=3us snapshot=250us ...`, every phase in order.
    pub fn format_line(frame: u64, micros: &[u64; TRACE_PHASES]) -> String {
        let mut s = format!("frame {frame}");
        for (p, us) in TracePhase::ALL.iter().zip(micros) {
            s.push_str(&format!(" {}={us}us", p.name()));
        }
        s
    }
}

/// Adds the time between its creation and its drop to a phase of the global trace.
pub struct TraceSpan {
    phase: TracePhase,
    start: Option<std::time::Instant>,
}

impl TraceSpan {
    pub fn new(phase: TracePhase) -> Self {
        Self {
            phase,
            start: FrameTrace::global().on().then(std::time::Instant::now),
        }
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            FrameTrace::global().add(self.phase, start.elapsed());
        }
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_the_value_1_turns_the_trace_on() {
        assert!(FrameTrace::enabled_by(Some("1")));
        for v in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!FrameTrace::enabled_by(v), "{v:?}");
        }
    }

    #[test]
    fn an_off_trace_records_nothing() {
        let t = FrameTrace::new(false);
        t.add(TracePhase::Scene, Duration::from_micros(500));
        assert_eq!(t.time(TracePhase::Paint, || 7), 7);
        assert_eq!(t.take(), [0; TRACE_PHASES]);
    }

    #[test]
    fn phases_add_up_and_take_resets() {
        let t = FrameTrace::new(true);
        t.add(TracePhase::Snapshot, Duration::from_micros(120));
        t.add(TracePhase::Snapshot, Duration::from_micros(30));
        t.add(TracePhase::Gap, Duration::from_micros(9));
        let got = t.take();
        assert_eq!(got[TracePhase::Snapshot as usize], 150);
        assert_eq!(got[TracePhase::Gap as usize], 9);
        assert_eq!(got.iter().sum::<u64>(), 159);
        assert_eq!(t.take(), [0; TRACE_PHASES]);
    }

    #[test]
    fn time_measures_the_closure() {
        let t = FrameTrace::new(true);
        t.time(TracePhase::Paint, || {
            std::thread::sleep(Duration::from_millis(3)); // sync-ok: test timing
        });
        assert!(t.take()[TracePhase::Paint as usize] >= 3000);
    }

    #[test]
    fn the_line_names_every_phase_in_microseconds() {
        let line = FrameTrace::format_line(4, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            line,
            "frame 4 lock_wait=1us snapshot=2us scene=3us glyph_upload=4us paint=5us set_image=6us render=7us gap=8us"
        );
        assert_eq!(TracePhase::ALL.len(), TRACE_PHASES);
        for (i, p) in TracePhase::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_none() {
        let s = FrameStats::new(10);
        assert_eq!(s.last(), None);
        assert_eq!(s.avg(), None);
        assert_eq!(s.max(), None);
        assert_eq!(s.p95(), None);
        assert_eq!(s.summary(), "no frames");
    }

    #[test]
    fn cap_drops_oldest() {
        let mut s = FrameStats::new(3);
        for v in [1.0, 2.0, 3.0, 4.0] {
            s.push_ms(v);
        }
        assert_eq!(s.avg(), Some(3.0));
        assert_eq!(s.last(), Some(4.0));
    }

    #[test]
    fn avg_and_max() {
        let mut s = FrameStats::new(10);
        s.push_ms(1.0);
        s.push_ms(5.0);
        s.push_ms(3.0);
        assert_eq!(s.avg(), Some(3.0));
        assert_eq!(s.max(), Some(5.0));
    }

    #[test]
    fn p95_nearest_rank() {
        let mut s = FrameStats::new(100);
        for i in 1..=20 {
            s.push_ms(f64::from(i));
        }
        assert_eq!(s.p95(), Some(19.0));

        let mut one = FrameStats::new(100);
        one.push_ms(5.0);
        assert_eq!(one.p95(), Some(5.0));
    }

    #[test]
    fn frame_stats_percentiles_match_a_known_series() {
        let mut s = FrameStats::new(200);
        for i in 1..=100 {
            s.push_ms(f64::from(i));
        }
        assert_eq!(s.percentile(50.0), Some(50.0));
        assert_eq!(s.percentile(95.0), Some(95.0));
        assert_eq!(s.percentile(99.0), Some(99.0));
        assert_eq!(s.max(), Some(100.0));
        assert_eq!(s.over(33.0), 67);
        assert_eq!(s.len(), 100);
        assert_eq!(FrameStats::new(5).percentile(50.0), None);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn the_ring_keeps_only_the_last_200000() {
        let mut s = FrameStats::new(200_000);
        for i in 0..200_005 {
            s.push_ms(f64::from(i));
        }
        assert_eq!(s.len(), 200_000);
        assert_eq!(s.percentile(0.0), Some(5.0));
        assert_eq!(s.max(), Some(200_004.0));
    }

    #[test]
    fn summary_format() {
        let mut s = FrameStats::new(10);
        s.push_ms(1.0);
        s.push_ms(2.0);
        assert_eq!(
            s.summary(),
            "last 2.00 ms, avg 1.50 ms, p95 2.00 ms, max 2.00 ms (n=2)"
        );
    }
}
