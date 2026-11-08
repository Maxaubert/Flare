//! The UI-thread side of the test hook: the one dispatcher for every request the pipe server hands
//! over (Tasks 8 to 11 fill its arms), and the render hooks the window's notifier calls (Appendix F).
//! Nothing here does anything unless `--e2e` is on.
//!
//! Task 8 owns the input arms: `Key`, `KeyPress`, `Text`, `Pointer`, `Drag`, `HoverFiles`, `DropFiles`,
//! `Resize`, `Focus`, `Quit`, `BlockUi`, `Slow`, `PanicWorker`, `Window`, `ClipSet`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flare_e2e_proto::{Button, PointerAction, Reply, Request, SnapMode};
use flare_render::stats::FrameStats;
use flare_term::keys::KeyEvent;
use flare_win::clipboard_port::ClipboardPort;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::winit_030::EventResult;
use slint::{ComponentHandle, LogicalPosition, SharedString};

use crate::AppWindow;
use crate::appctl::{AppCtl, CloseDecision};
use crate::config::Args;
use crate::e2e_hook::Responder;
use crate::window::KeyPath;

/// Which side of a rendered frame the caller is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPhase {
    Before,
    After,
}

/// Called by the render path around every frame. Returns at once unless `--e2e` is on. Records the
/// frame series and runs the armed latency and prompt watchers (Task 10).
pub fn on_render(phase: RenderPhase) {
    if !crate::e2e::global().on() {
        return;
    }
    let now = Instant::now();
    let mut to_press: Vec<String> = Vec::new();
    let mut env: Option<Rc<dyn FrameEnv>> = None;
    FRAMES.with(|f| {
        let Ok(mut st) = f.try_borrow_mut() else {
            return;
        };
        match phase {
            RenderPhase::Before => st.rec.before(now),
            RenderPhase::After => {
                env = st.env.clone();
                if let Some(e) = env.clone() {
                    let age = crate::startup_trace::since_process_start().as_secs_f64() * 1000.0;
                    to_press = st.after(now, age, e.as_ref());
                } else {
                    st.rec.after(now);
                }
            }
        }
    });
    // The key goes in from the event loop, never from inside a render callback.
    for chord in to_press {
        if let Some(e) = env.clone() {
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Err(m) = e.press_down(&chord) {
                    log::warn!("e2e: the prompt key was not injected: {m}");
                }
            });
        }
    }
}

thread_local! {
    /// Everything the render hook keeps; UI thread only, and only filled under `--e2e`.
    static FRAMES: RefCell<FrameState> = RefCell::new(FrameState::new());
}

// ---- frames, snapshots, latency (Task 10) --------------------------------------------------

fn ms_between(later: Instant, earlier: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64() * 1000.0
}

/// The most frames the series keeps (about eight minutes at 60 fps); later frames are not kept.
pub const SERIES_CAP: usize = 30_000;

/// One presented frame, for the visual tour (audit 2026-10-20): when it was (ms since the process
/// started), the gap since the frame before, the time it took, and the sidebar's animated slot width
/// and panel x at that frame. A value the frame did not have is `None`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub age_ms: f64,
    pub gap_ms: Option<f64>,
    pub render_ms: Option<f64>,
    pub slot: Option<f32>,
    pub panel_left: Option<f32>,
}

/// The frame series: the gap between frames and the time each frame took.
pub struct FrameRecorder {
    gaps: FrameStats,
    renders: FrameStats,
    before: Option<Instant>,
    last_after: Option<Instant>,
    /// The gap and render time of the frame `after` just closed, for the series.
    cur: (Option<f64>, Option<f64>),
    series: Vec<FrameSample>,
    /// The software renderer has no rendering notifier, so its frames are the window's redraw requests
    /// (`redraw_sample`), and a grid render adds nothing to the series.
    redraw_series: bool,
    last_redraw: Option<Instant>,
    on: bool,
}

impl Default for FrameRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRecorder {
    pub fn new() -> Self {
        Self {
            gaps: FrameStats::new(200_000),
            renders: FrameStats::new(200_000),
            before: None,
            last_after: None,
            cur: (None, None),
            series: Vec::new(),
            redraw_series: false,
            last_redraw: None,
            on: false,
        }
    }

    /// Recording of the statistics starts at the first `Frames` or `Latency` request (the series
    /// needs no request: it runs from the first frame, so start-up is in it).
    pub fn start(&mut self) {
        self.on = true;
    }

    pub fn reset(&mut self) {
        self.gaps.clear();
        self.renders.clear();
        self.before = None;
        self.last_after = None;
        self.cur = (None, None);
        self.series.clear();
        self.last_redraw = None;
    }

    /// From now on the series is the window's redraw requests (the software renderer).
    pub fn use_redraw_series(&mut self) {
        self.redraw_series = true;
    }

    /// One redraw request of the window, with the slide it will show (software renderer only).
    pub fn redraw_sample(&mut self, now: Instant, age_ms: f64, slide: Option<(f32, f32)>) {
        if !self.redraw_series || self.series.len() >= SERIES_CAP {
            return;
        }
        let gap = self.last_redraw.map(|p| ms_between(now, p));
        self.last_redraw = Some(now);
        self.series.push(FrameSample {
            age_ms,
            gap_ms: gap,
            render_ms: None,
            slot: slide.map(|s| s.0),
            panel_left: slide.map(|s| s.1),
        });
    }

    pub fn before(&mut self, now: Instant) {
        self.before = Some(now);
    }

    pub fn after(&mut self, now: Instant) {
        let render = self.before.take().map(|b| ms_between(now, b));
        let gap = self.last_after.map(|p| ms_between(now, p));
        self.last_after = Some(now);
        self.cur = (gap, render);
        if !self.on {
            return;
        }
        if let Some(r) = render {
            self.renders.push_ms(r);
        }
        if let Some(g) = gap {
            self.gaps.push_ms(g);
        }
    }

    /// Adds the frame `after` just closed to the series, with the slide it showed.
    pub fn sample(&mut self, age_ms: f64, slide: Option<(f32, f32)>) {
        if self.redraw_series || self.series.len() >= SERIES_CAP {
            return;
        }
        self.series.push(FrameSample {
            age_ms,
            gap_ms: self.cur.0,
            render_ms: self.cur.1,
            slot: slide.map(|s| s.0),
            panel_left: slide.map(|s| s.1),
        });
    }

    /// Every frame since the last reset.
    #[cfg(test)]
    pub fn series(&self) -> &[FrameSample] {
        &self.series
    }

    /// The series as rows `[age_ms, gap_ms, render_ms, slot, panel_left]` (null for a missing value).
    pub fn series_json(&self) -> serde_json::Value {
        let rows: Vec<serde_json::Value> = self
            .series
            .iter()
            .map(|s| serde_json::json!([s.age_ms, s.gap_ms, s.render_ms, s.slot, s.panel_left]))
            .collect();
        serde_json::Value::Array(rows)
    }

    /// The statistics of everything since the last reset.
    pub fn reply(&self) -> Reply {
        let z = |v: Option<f64>| v.unwrap_or(0.0);
        Reply::Frames {
            count: u32::try_from(self.renders.len()).unwrap_or(u32::MAX),
            mean_ms: z(self.gaps.avg()),
            p50_ms: z(self.gaps.percentile(50.0)),
            p95_ms: z(self.gaps.percentile(95.0)),
            p99_ms: z(self.gaps.percentile(99.0)),
            max_ms: z(self.gaps.max()),
            over_33: u32::try_from(self.gaps.over(33.0)).unwrap_or(u32::MAX),
            render_p95_ms: z(self.renders.percentile(95.0)),
        }
    }
}

/// What a frame shows of one tab: every screen row, and the row the cursor is on (trimmed).
#[derive(Debug, Clone, Default)]
pub struct FrameView {
    pub text: String,
    pub cursor_row: String,
}

/// What the render hook needs from the running app; a fake stands in for it in the tests.
pub trait FrameEnv {
    fn view(&self, tab: Option<&str>) -> Result<FrameView, String>;
    fn press_down(&self, chord: &str) -> Result<(), String>;
    /// The sidebar's animated slot width and panel x (css px) as this frame shows them.
    fn slide(&self) -> Option<(f32, f32)> {
        None
    }
}

/// A key-to-frame measurement: done at the first frame that holds the text.
#[derive(Debug, Clone)]
pub struct LatencyWatch {
    t0: Instant,
    expect: String,
    deadline: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LatencyStep {
    Waiting,
    Done(f64),
    TimedOut,
}

/// The longest wait a pipe request may ask for (ten minutes). A larger one would overflow the
/// deadline `Instant` and panic the app (L59).
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// The most moves one `Drag` request may make; more would build an event list without bound (L59).
pub const MAX_DRAG_STEPS: u32 = 2_000;

pub fn clamp_timeout_ms(ms: u64) -> u64 {
    ms.min(MAX_TIMEOUT_MS)
}

impl LatencyWatch {
    pub fn new(t0: Instant, expect: &str, timeout_ms: u64) -> Self {
        Self {
            t0,
            expect: expect.to_string(),
            deadline: t0 + Duration::from_millis(clamp_timeout_ms(timeout_ms)),
        }
    }

    pub fn frame(&self, now: Instant, text: &str) -> LatencyStep {
        if text.contains(&self.expect) {
            LatencyStep::Done(ms_between(now, self.t0))
        } else if now >= self.deadline {
            LatencyStep::TimedOut
        } else {
            LatencyStep::Waiting
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptStep {
    Waiting,
    /// The prompt is up: inject the key now.
    Inject,
    Done {
        prompt_ms: f64,
        usable_ms: f64,
    },
}

#[derive(Debug, Clone)]
enum PromptStage {
    WaitPrompt,
    WaitEcho { row: String, prompt_ms: f64 },
}

/// B1: waits for the prompt, asks for the key once, then waits for its echo.
#[derive(Debug, Clone)]
pub struct PromptWatch {
    suffix: String,
    echo: Option<String>,
    stage: PromptStage,
}

impl PromptWatch {
    /// `echo` is the text the key types (None: any change of the row counts).
    pub fn new(prompt_suffix: &str, echo: Option<String>) -> Self {
        Self {
            // The cursor row comes back trimmed at the right.
            suffix: prompt_suffix.trim_end().to_string(),
            echo,
            stage: PromptStage::WaitPrompt,
        }
    }

    /// One finished frame: the cursor row and the process age in ms.
    pub fn frame(&mut self, row: &str, age_ms: f64) -> PromptStep {
        match &self.stage {
            PromptStage::WaitPrompt => {
                if row.ends_with(&self.suffix) {
                    self.stage = PromptStage::WaitEcho {
                        row: row.to_string(),
                        prompt_ms: age_ms,
                    };
                    PromptStep::Inject
                } else {
                    PromptStep::Waiting
                }
            }
            PromptStage::WaitEcho {
                row: before,
                prompt_ms,
            } => {
                let echoed = row != before && self.echo.as_ref().is_none_or(|e| row.ends_with(e));
                if echoed {
                    PromptStep::Done {
                        prompt_ms: *prompt_ms,
                        usable_ms: age_ms,
                    }
                } else {
                    PromptStep::Waiting
                }
            }
        }
    }
}

struct PendingLatency {
    watch: LatencyWatch,
    tab: Option<String>,
    resp: Responder,
    seq: u64,
}

/// What the render hook keeps between frames.
pub struct FrameState {
    rec: FrameRecorder,
    latency: Option<PendingLatency>,
    prompt: Option<(PromptWatch, String)>,
    seq: u64,
    snap: Option<Snap>,
    /// The sidebar's slot and panel x read right after the last snapshot's picture was taken, so a
    /// label made from it names the frame the picture shows (not the frame before or after).
    snap_slide: Option<(f32, f32)>,
    /// The breathing marks' opacity read right after the last snapshot's picture (1.0 when the window
    /// is still): a question dot or line in the picture is drawn at this strength, so a scenario can
    /// tell a faded breath from a missing mark.
    snap_breathe: Option<f32>,
    marks: (Option<f64>, Option<f64>),
    env: Option<Rc<dyn FrameEnv>>,
}

impl Default for FrameState {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameState {
    pub fn new() -> Self {
        Self {
            rec: FrameRecorder::new(),
            latency: None,
            prompt: None,
            seq: 0,
            snap: None,
            snap_slide: None,
            snap_breathe: None,
            marks: (None, None),
            env: None,
        }
    }

    /// Keeps the slide the last snapshot's picture shows.
    pub fn set_snap_slide(&mut self, slide: Option<(f32, f32)>) {
        self.snap_slide = slide;
    }

    /// The last picture's slide as `[slot, panel_left]`, or null when it had none.
    pub fn snap_slide_json(&self) -> serde_json::Value {
        match self.snap_slide {
            Some((slot, left)) => serde_json::json!([slot, left]),
            None => serde_json::Value::Null,
        }
    }

    /// Keeps the breath the last snapshot's picture shows.
    pub fn set_snap_breathe(&mut self, breathe: Option<f32>) {
        self.snap_breathe = breathe;
    }

    /// The last picture's breath (0 to 1), or null when it had none.
    pub fn snap_breathe_json(&self) -> serde_json::Value {
        match self.snap_breathe {
            Some(b) => serde_json::json!(b),
            None => serde_json::Value::Null,
        }
    }

    /// `Frames { reset }`.
    pub fn frames_request(&mut self, reset: bool) -> Reply {
        self.rec.start();
        if reset {
            self.rec.reset();
            Reply::Ok
        } else {
            self.rec.reply()
        }
    }

    /// Arms a latency measurement; one already waiting is answered with an error.
    fn arm_latency(&mut self, watch: LatencyWatch, tab: Option<String>, resp: Responder) -> u64 {
        self.rec.start();
        if let Some(old) = self.latency.take() {
            old.resp.reply(err("superseded by a newer Latency request"));
        }
        self.seq += 1;
        self.latency = Some(PendingLatency {
            watch,
            tab,
            resp,
            seq: self.seq,
        });
        self.seq
    }

    /// The timeout timer: answers the measurement `seq` with an error if it is still waiting.
    fn latency_timed_out(&mut self, seq: u64) {
        if self.latency.as_ref().is_some_and(|p| p.seq == seq)
            && let Some(p) = self.latency.take()
        {
            p.resp.reply(err("latency: the text never reached a frame"));
        }
    }

    /// One finished frame. Returns the keys to inject (from the event loop).
    pub fn after(&mut self, now: Instant, age_ms: f64, env: &dyn FrameEnv) -> Vec<String> {
        self.rec.after(now);
        self.rec.sample(age_ms, env.slide());
        if let Some(p) = self.latency.take() {
            match env.view(p.tab.as_deref()) {
                Err(m) => p.resp.reply(err(m)),
                Ok(v) => match p.watch.frame(now, &v.text) {
                    LatencyStep::Done(ms) => p.resp.reply(Reply::Latency { ms }),
                    LatencyStep::TimedOut => {
                        p.resp.reply(err("latency: the text never reached a frame"))
                    }
                    LatencyStep::Waiting => self.latency = Some(p),
                },
            }
        }
        let mut press = Vec::new();
        if let Some((watch, chord)) = self.prompt.as_mut() {
            let row = env.view(None).map(|v| v.cursor_row).unwrap_or_default();
            match watch.frame(&row, age_ms) {
                PromptStep::Waiting => {}
                PromptStep::Inject => {
                    self.marks.0 = Some(age_ms);
                    press.push(chord.clone());
                }
                PromptStep::Done {
                    prompt_ms,
                    usable_ms,
                } => {
                    self.marks = (Some(prompt_ms), Some(usable_ms));
                    self.prompt = None;
                }
            }
        }
        press
    }
}

/// `prompt_ms` and `usable_ms` of the last `KeyAtPrompt` (None until known), for `startup.trace`.
pub fn prompt_marks() -> (Option<f64>, Option<f64>) {
    FRAMES.with(|f| f.try_borrow().map(|s| s.marks).unwrap_or((None, None)))
}

/// A window's pixels, straight RGBA8, device pixels.
#[derive(Debug, Clone)]
pub struct Snap {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Snap {
    /// True for a buffer with no pixels or one value in every pixel.
    pub fn is_blank(&self) -> bool {
        let (pixels, _) = self.rgba.as_chunks::<4>();
        match pixels.first() {
            None => true,
            Some(first) => pixels.iter().all(|p| p == first),
        }
    }

    pub fn px(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let i = (y as usize * self.w as usize + x as usize) * 4;
        self.rgba.get(i..i + 4).map(|p| [p[0], p[1], p[2], p[3]])
    }
}

/// What `Auto` does with Slint's own snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapPick {
    UseSlint,
    PrintWindow,
}

/// The `Auto` chooser: Slint's snapshot unless it failed or came back all one value.
pub fn pick_snapshot(slint: &Result<Snap, String>) -> SnapPick {
    match slint {
        Ok(s) if !s.is_blank() => SnapPick::UseSlint,
        _ => SnapPick::PrintWindow,
    }
}

/// A PNG of the pixels.
pub fn encode_png(s: &Snap) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, s.w, s.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&s.rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(
        hwnd: windows_sys::Win32::Foundation::HWND,
        hdc: windows_sys::Win32::Graphics::Gdi::HDC,
        flags: u32,
    ) -> i32;
}

/// `PrintWindow` with `PW_RENDERFULLCONTENT` into a 32-bit DIB: works for a swapchain while the
/// window is parked off every monitor.
#[cfg(windows)]
fn print_window_snapshot(hwnd: isize) -> Result<Snap, String> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, SelectObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    const PW_RENDERFULLCONTENT: u32 = 2;
    let h = hwnd as HWND;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: a window of this process and a valid out pointer.
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        return Err("PrintWindow: no window rectangle".into());
    }
    let (w, hh) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || hh <= 0 {
        return Err("PrintWindow: the window is empty".into());
    }
    // SAFETY: plain GDI calls; every object made here is released before returning, and the DIB's
    // memory is only read while the bitmap is alive.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return Err("PrintWindow: no device context".into());
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -hh,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return Err("PrintWindow: no bitmap".into());
        }
        let old = SelectObject(dc, bmp);
        let ok = PrintWindow(h, dc, PW_RENDERFULLCONTENT);
        let n = w as usize * hh as usize * 4;
        let src = std::slice::from_raw_parts(bits as *const u8, n);
        let mut rgba = Vec::with_capacity(n);
        for p in src.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        if ok == 0 {
            return Err("PrintWindow failed".into());
        }
        Ok(Snap {
            w: w as u32,
            h: hh as u32,
            rgba,
        })
    }
}

/// The strength a breathing mark is drawn at: the clock's breath, or full when the window is still
/// (tabmark.slint draws a still question at opacity 1 whatever `breathe` last held).
fn shown_breathe(still: bool, breathe: f32) -> f32 {
    if still { 1.0 } else { breathe }
}

/// Slint's own snapshot of the window.
fn slint_snapshot(window: &slint::Window) -> Result<Snap, String> {
    let buf = window.take_snapshot().map_err(|e| e.to_string())?;
    let mut rgba = buf.as_bytes().to_vec();
    // The window's pixels come back premultiplied (a see-through ground at alpha 153 holds 23 * 153 / 255 =
    // 14), but `Snap` and the PNG are straight RGBA: convert, or the sheet shows the glass twice as dark.
    unpremultiply(&mut rgba);
    Ok(Snap {
        w: buf.width(),
        h: buf.height(),
        rgba,
    })
}

/// Premultiplied RGBA8 to straight, in place. Alpha 0 becomes clear black; alpha 255 is untouched.
pub fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        match p[3] {
            255 => {}
            0 => *p = [0; 4],
            a => {
                for c in &mut p[..3] {
                    *c = (u32::from(*c) * 255 / u32::from(a)).min(255) as u8;
                }
            }
        }
    }
}

thread_local! {
    /// The running hook server; dropped (and its pipe closed) with the UI thread.
    static HOOK: RefCell<Option<crate::e2e_hook::HookServer>> = const { RefCell::new(None) };
}

// ---- the chord grammar ---------------------------------------------------------------------

/// The W3C code of a printable character, and whether typing it needs Shift.
fn code_of_char(c: char) -> Option<(String, bool)> {
    let shifted_digit = |d: u32| format!("Digit{d}");
    Some(match c {
        'a'..='z' => (format!("Key{}", c.to_ascii_uppercase()), false),
        'A'..='Z' => (format!("Key{c}"), true),
        '0'..='9' => (format!("Digit{c}"), false),
        ',' => ("Comma".into(), false),
        '.' => ("Period".into(), false),
        '/' => ("Slash".into(), false),
        ';' => ("Semicolon".into(), false),
        '\'' => ("Quote".into(), false),
        '[' => ("BracketLeft".into(), false),
        ']' => ("BracketRight".into(), false),
        '\\' => ("Backslash".into(), false),
        '-' => ("Minus".into(), false),
        '=' => ("Equal".into(), false),
        '`' => ("Backquote".into(), false),
        ' ' => ("Space".into(), false),
        '!' => (shifted_digit(1), true),
        '@' => (shifted_digit(2), true),
        '#' => (shifted_digit(3), true),
        '$' => (shifted_digit(4), true),
        '%' => (shifted_digit(5), true),
        '^' => (shifted_digit(6), true),
        '&' => (shifted_digit(7), true),
        '*' => (shifted_digit(8), true),
        '(' => (shifted_digit(9), true),
        ')' => (shifted_digit(0), true),
        '?' => ("Slash".into(), true),
        ':' => ("Semicolon".into(), true),
        '"' => ("Quote".into(), true),
        '<' => ("Comma".into(), true),
        '>' => ("Period".into(), true),
        '_' => ("Minus".into(), true),
        '+' => ("Equal".into(), true),
        '{' => ("BracketLeft".into(), true),
        '}' => ("BracketRight".into(), true),
        '|' => ("Backslash".into(), true),
        '~' => ("Backquote".into(), true),
        _ => return None,
    })
}

/// True when `t` names a W3C code this grammar accepts.
fn is_code(t: &str) -> bool {
    const NAMED: [&str; 32] = [
        "Enter",
        "Tab",
        "Space",
        "Escape",
        "Backspace",
        "Delete",
        "Insert",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "PageUp",
        "PageDown",
        "Home",
        "End",
        "Comma",
        "Period",
        "Slash",
        "Semicolon",
        "Quote",
        "BracketLeft",
        "BracketRight",
        "Backslash",
        "Minus",
        "Equal",
        "Backquote",
        "NumpadEnter",
        "CapsLock",
        // A modifier on its own: a held Ctrl is let go with its key-up, which ends a walk.
        "ControlLeft",
        "ShiftLeft",
        "AltLeft",
        // Right Alt alone (M8: the dictation key is Right Alt in the scenarios that hold it).
        "AltRight",
    ];
    if NAMED.contains(&t) {
        return true;
    }
    if let Some(l) = t.strip_prefix("Key") {
        return l.len() == 1 && l.chars().all(|c| c.is_ascii_uppercase());
    }
    if let Some(d) = t.strip_prefix("Digit") {
        return d.len() == 1 && d.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(n) = t.strip_prefix('F') {
        return n.parse::<u32>().is_ok_and(|n| (1..=24).contains(&n));
    }
    if let Some(n) = t.strip_prefix("Numpad") {
        return n.len() == 1 && n.chars().all(|c| c.is_ascii_digit())
            || ["Add", "Subtract", "Multiply", "Divide", "Decimal"].contains(&n);
    }
    false
}

/// The logical key name of a code (what a US layout would report), `shift` applied.
fn logical_of_code(code: &str, shift: bool) -> String {
    if let Some(l) = code.strip_prefix("Key") {
        return if shift {
            l.to_string()
        } else {
            l.to_lowercase()
        };
    }
    if let Some(d) = code.strip_prefix("Digit") {
        let d = d.chars().next().unwrap_or('0');
        return if shift {
            ")!@#$%^&*("
                .chars()
                .nth(d as usize - '0' as usize)
                .unwrap_or(d)
        } else {
            d
        }
        .to_string();
    }
    if let Some(n) = code.strip_prefix("Numpad") {
        return match n {
            "Enter" => "Enter".into(),
            "Add" => "+".into(),
            "Subtract" => "-".into(),
            "Multiply" => "*".into(),
            "Divide" => "/".into(),
            "Decimal" => ".".into(),
            d => d.into(),
        };
    }
    let pair = |plain: &str, shifted: &str| if shift { shifted } else { plain }.to_string();
    match code {
        "Space" => " ".into(),
        "ControlLeft" => "Control".into(),
        "ShiftLeft" => "Shift".into(),
        "AltLeft" | "AltRight" => "Alt".into(),
        "Comma" => pair(",", "<"),
        "Period" => pair(".", ">"),
        "Slash" => pair("/", "?"),
        "Semicolon" => pair(";", ":"),
        "Quote" => pair("'", "\""),
        "BracketLeft" => pair("[", "{"),
        "BracketRight" => pair("]", "}"),
        "Backslash" => pair("\\", "|"),
        "Minus" => pair("-", "_"),
        "Equal" => pair("=", "+"),
        "Backquote" => pair("`", "~"),
        other => other.to_string(),
    }
}

/// True for a code that types a printable character.
fn is_printable_code(code: &str) -> bool {
    code.starts_with("Key")
        || code.starts_with("Digit")
        || code == "Space"
        || (code.starts_with("Numpad") && code != "NumpadEnter")
        || [
            "Comma",
            "Period",
            "Slash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Minus",
            "Equal",
            "Backquote",
        ]
        .contains(&code)
}

/// Where the optional `=<logical key>` starts: the first `=` that is not the key itself
/// (`Ctrl+=` is the equals key).
fn logical_split(chord: &str) -> Option<usize> {
    let mut prev: Option<char> = None;
    for (i, c) in chord.char_indices() {
        if c == '=' && prev.is_some_and(|p| p != '+') {
            return Some(i);
        }
        prev = Some(c);
    }
    None
}

/// One key event from a chord: modifiers joined by `+`, then a W3C code or one printable
/// character, then an optional `=<logical key>`. The fields are exactly what
/// `keymap::key_event_from_parts` builds.
pub fn event_from_chord(chord: &str, down: bool) -> Result<KeyEvent, String> {
    let (body, logical_override) = match logical_split(chord) {
        Some(i) => (&chord[..i], Some(&chord[i + 1..])),
        None => (chord, None),
    };
    if body.is_empty() {
        return Err("empty chord".into());
    }
    // A key that is itself a plus sign: "+" or "Ctrl++".
    let (mod_part, key_token) = if body == "+" {
        ("", "+")
    } else if let Some(head) = body.strip_suffix("++") {
        (head, "+")
    } else {
        match body.rfind('+') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => ("", body),
        }
    };
    if key_token.is_empty() {
        return Err(format!("chord {chord:?} ends in a plus sign"));
    }
    let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
    if !mod_part.is_empty() {
        for m in mod_part.split('+') {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "meta" => meta = true,
                _ => return Err(format!("unknown modifier {m:?} in {chord:?}")),
            }
        }
    }
    let (code, implied_shift) = if is_code(key_token) {
        (key_token.to_string(), false)
    } else if key_token.chars().count() == 1 {
        // A letter in a chord names the key; Shift is a modifier of its own.
        let c = key_token.chars().next().unwrap_or(' ').to_ascii_lowercase();
        code_of_char(c).ok_or_else(|| format!("no key for the character {key_token:?}"))?
    } else {
        return Err(format!("unknown key {key_token:?} in {chord:?}"));
    };
    shift |= implied_shift;
    let key = match logical_override {
        Some(l) if !l.is_empty() => l.to_string(),
        Some(_) => return Err(format!("empty logical key in {chord:?}")),
        None => logical_of_code(&code, shift),
    };
    let text = (is_printable_code(&code) && !(ctrl && !alt)).then(|| key.clone());
    Ok(KeyEvent {
        pressed: down,
        key,
        code,
        text,
        ctrl,
        shift,
        alt,
        meta,
    })
}

/// One typed character as a key press: `\n` is Enter, a character on the keyboard keeps its
/// code, anything else (an accent, an emoji) arrives as text only.
pub fn event_from_char(c: char, down: bool) -> KeyEvent {
    if c == '\n' {
        return KeyEvent {
            pressed: down,
            ..KeyEvent::new("Enter", "Enter")
        };
    }
    match code_of_char(c) {
        Some((code, shift)) => KeyEvent {
            pressed: down,
            key: c.to_string(),
            code,
            text: Some(c.to_string()),
            ctrl: false,
            shift,
            alt: false,
            meta: false,
        },
        None => KeyEvent {
            pressed: down,
            key: c.to_string(),
            code: "Unidentified".into(),
            text: Some(c.to_string()),
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        },
    }
}

/// What Slint should see for a key the app left to it: the key's text, or Slint's own character
/// for a named key.
pub fn slint_text_for(ev: &KeyEvent) -> Option<SharedString> {
    use slint::platform::Key;
    if let Some(t) = &ev.text {
        return Some(SharedString::from(t.as_str()));
    }
    let k = match ev.key.as_str() {
        "Enter" => Key::Return,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        _ => return None,
    };
    Some(SharedString::from(char::from(k)))
}

/// What Slint should see for one key it was left: the modifiers held (Slint tracks Shift, Control,
/// Alt and Meta from their own key events), then the key; on release the key, then the modifiers
/// let go. A Ctrl or Alt chord on a character sends the character, so Ctrl+A is select-all there.
/// Nothing when Slint has no text for the key.
pub fn slint_key_events(ev: &KeyEvent) -> Vec<WindowEvent> {
    use slint::platform::Key;
    let text = slint_text_for(ev).or_else(|| {
        let mut chars = ev.key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if ev.ctrl || ev.alt || ev.meta => {
                Some(SharedString::from(c.to_string()))
            }
            _ => None,
        }
    });
    let Some(text) = text else {
        return Vec::new();
    };
    let held: Vec<SharedString> = [
        (ev.shift, Key::Shift),
        (ev.ctrl, Key::Control),
        (ev.alt, Key::Alt),
        (ev.meta, Key::Meta),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, k)| SharedString::from(char::from(k)))
    .collect();
    let down = |t: SharedString| WindowEvent::KeyPressed { text: t };
    let up = |t: SharedString| WindowEvent::KeyReleased { text: t };
    if ev.pressed {
        let mut out: Vec<WindowEvent> = held.into_iter().map(down).collect();
        out.push(down(text));
        out
    } else {
        let mut out = vec![up(text)];
        out.extend(held.into_iter().rev().map(up));
        out
    }
}

/// The key-up of each modifier a chord holds, so Slint never keeps one pressed after the chord.
pub fn slint_modifier_releases(ev: &KeyEvent) -> Vec<WindowEvent> {
    use slint::platform::Key;
    [
        (ev.shift, Key::Shift),
        (ev.ctrl, Key::Control),
        (ev.alt, Key::Alt),
        (ev.meta, Key::Meta),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, k)| WindowEvent::KeyReleased {
        text: SharedString::from(char::from(k)),
    })
    .collect()
}
// ---- pointer events (pure) -----------------------------------------------------------------

fn button(b: Button) -> PointerEventButton {
    match b {
        Button::Left => PointerEventButton::Left,
        Button::Right => PointerEventButton::Right,
        Button::Middle => PointerEventButton::Middle,
    }
}

/// The Slint events of one pointer action at a logical position.
pub fn pointer_events(x: f32, y: f32, action: &PointerAction) -> Vec<WindowEvent> {
    let position = LogicalPosition::new(x, y);
    let moved = || WindowEvent::PointerMoved { position };
    match action {
        PointerAction::Move => vec![moved()],
        PointerAction::Down { button: b } => vec![
            moved(),
            WindowEvent::PointerPressed {
                position,
                button: button(*b),
            },
        ],
        PointerAction::Up { button: b } => vec![WindowEvent::PointerReleased {
            position,
            button: button(*b),
        }],
        PointerAction::Click { button: b, count } => (0..(*count).max(1))
            .flat_map(|_| {
                [
                    moved(),
                    WindowEvent::PointerPressed {
                        position,
                        button: button(*b),
                    },
                    WindowEvent::PointerReleased {
                        position,
                        button: button(*b),
                    },
                ]
            })
            .collect(),
        PointerAction::Wheel { dx, dy } => vec![
            moved(),
            WindowEvent::PointerScrolled {
                position,
                delta_x: *dx,
                delta_y: *dy,
            },
        ],
        PointerAction::Leave => vec![WindowEvent::PointerExited],
    }
}

/// The wheel lines an injected wheel of `dy` css px makes (positive is up, winit's convention): a
/// notch is 40 px. It goes to the shared wheel as lines, so a small movement adds up in the
/// accumulator and is never rounded up to a notch (audit 2026-10-20: M6); only the size is capped.
pub fn wheel_lines(dy: f32) -> f32 {
    (dy / 40.0).clamp(-4096.0, 4096.0)
}

/// The placement a restore asks for: the normal size and place, shown WITHOUT activation, so a
/// restore from the pipe can never take the foreground from the user's window (L58).
#[cfg(windows)]
fn restore_placement(
    mut p: windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT,
) -> windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT {
    p.showCmd = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE as u32;
    p
}

/// Restores a minimised or maximised window with `SetWindowPlacement(SW_SHOWNOACTIVATE)`; the
/// Slint calls (`set_minimized(false)`, `set_maximized(false)`) end in `SW_RESTORE`, which activates.
#[cfg(windows)]
fn restore_without_activating(app: &AppWindow) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, IsIconic, SW_SHOWNOACTIVATE, SetWindowPlacement, ShowWindow,
        WINDOWPLACEMENT,
    };
    let hwnd = crate::window::hwnd_of(app).ok_or("the window has no handle")?;
    let h = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: WINDOWPLACEMENT is plain data.
    let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    p.length = size_of::<WINDOWPLACEMENT>() as u32;
    // SAFETY: `h` is this process's window and `p` is a sized out pointer.
    if unsafe { GetWindowPlacement(h, &mut p) } == 0 {
        return Err("GetWindowPlacement failed".into());
    }
    let p = restore_placement(p);
    // SAFETY: as above; `p` is a fully read placement.
    if unsafe { SetWindowPlacement(h, &p) } == 0 {
        return Err("SetWindowPlacement failed".into());
    }
    // A minimised window can stay minimised under the placement call alone; this shows it, still
    // without activation.
    // SAFETY: as above.
    if unsafe { IsIconic(h) } != 0 {
        // SAFETY: as above; shows the window in place without activating it.
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
    }
    Ok(())
}

/// Press at `from`, `steps` equal moves, release at `to`.
pub fn drag_events(from: (f32, f32), to: (f32, f32), steps: u32, b: Button) -> Vec<WindowEvent> {
    let steps = steps.min(MAX_DRAG_STEPS);
    let mut out = pointer_events(from.0, from.1, &PointerAction::Down { button: b });
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        out.extend(pointer_events(
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
            &PointerAction::Move,
        ));
    }
    out.extend(pointer_events(to.0, to.1, &PointerAction::Up { button: b }));
    out
}

// ---- drops ---------------------------------------------------------------------------------

/// What a file drop calls: winit's `HoveredFile` and `DroppedFile` end in these two.
pub trait DropTarget {
    fn hovered(&mut self);
    fn dropped(&mut self, path: PathBuf, at: (f32, f32));
}

impl DropTarget for AppCtl {
    fn hovered(&mut self) {
        self.on_hovered_file();
    }
    fn dropped(&mut self, path: PathBuf, at: (f32, f32)) {
        self.on_dropped_path(path, at);
    }
}

/// A file hovers the window and nothing is dropped (winit's `HoveredFile` alone): a drag in flight.
pub fn hover_files(target: &mut dyn DropTarget) {
    target.hovered();
}

/// Whether the controller hears the injected cursor: a collapsed sidebar is revealed by it, and a
/// file drag in flight ends on the next move (the real window tells it with every `CursorMoved`).
pub fn cursor_listening(sidebar_collapsed: bool, drag_in_flight: bool) -> bool {
    sidebar_collapsed || drag_in_flight
}

/// Hovered, then each path dropped at `at`.
pub fn drop_files(target: &mut dyn DropTarget, paths: &[String], at: (f32, f32)) {
    target.hovered();
    for p in paths {
        target.dropped(PathBuf::from(p), at);
    }
}

// ---- the dispatcher ------------------------------------------------------------------------

/// What the dispatcher needs from the running window.
pub struct GlueCtx {
    app: slint::Weak<AppWindow>,
    ctl: std::rc::Weak<RefCell<AppCtl>>,
    kp: Rc<KeyPath>,
    clip: Option<Arc<dyn ClipboardPort>>,
    args: Args,
    probes: crate::e2e_probes::ProbeRegistry,
}

fn err(message: impl Into<String>) -> Reply {
    Reply::Err {
        message: message.into(),
    }
}

/// What the real window tells the controller about the cursor that an injected Slint pointer event
/// does not: winit's `CursorMoved` and `CursorLeft`. Only a collapsed sidebar listens (it is revealed
/// by the cursor), so every other scenario keeps the events it always had. `Some(Some(p))` is a move
/// to `p` in css px, `Some(None)` the cursor leaving, `None` nothing to tell.
pub fn cursor_forward(e: &WindowEvent, sidebar_collapsed: bool) -> Option<Option<(f32, f32)>> {
    if !sidebar_collapsed {
        return None;
    }
    match e {
        WindowEvent::PointerMoved { position } => Some(Some((position.x, position.y))),
        WindowEvent::PointerExited => Some(None),
        _ => None,
    }
}

impl GlueCtx {
    /// `WebScript`: starts a script in the front tab's page (the answer is a probe).
    fn web_script(&self, js: &str) -> Result<(), String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        c.web_script(js)
            .map(|_| ())
            .ok_or_else(|| "the front tab has no web page".to_string())
    }

    /// `WebZoom`: the browser zoom of the front web page.
    fn web_zoom(&self, factor: f64) -> Result<(), String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        if c.web_zoom(factor) {
            Ok(())
        } else {
            Err("the front tab has no web page".to_string())
        }
    }

    /// `ToastClick`: the person clicked a notification about this tab.
    fn toast_click(&self, tab: &str) -> Result<(), String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        c.e2e_toast_click(tab);
        Ok(())
    }

    /// `WebUserInput`: the person took over the front web page.
    fn web_user_input(&self) -> Result<(), String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        c.web_user_input();
        Ok(())
    }

    fn dispatch(&self, events: Vec<WindowEvent>) -> Result<(), String> {
        let app = self.app.upgrade().ok_or("the window is gone")?;
        for e in events {
            let in_flight = self
                .ctl
                .upgrade()
                .and_then(|rc| rc.try_borrow().ok().map(|c| c.drag_in_flight()))
                .unwrap_or(false);
            if let Some(told) =
                cursor_forward(&e, cursor_listening(app.get_sidebar_collapsed(), in_flight))
                && let Some(rc) = self.ctl.upgrade()
                && let Ok(mut c) = rc.try_borrow_mut()
            {
                match told {
                    Some((x, y)) => c.on_cursor(x, y),
                    None => c.on_cursor_left(),
                }
            }
            app.window().dispatch_event(e);
        }
        Ok(())
    }

    /// An injected Slint wheel never reaches winit's `MouseWheel`, where the real window scrolls the
    /// terminal. It takes the same path the real one does (`KeyPath::wheel`: the grid gate, the
    /// accumulated notches, the cell under the pointer, the redraw), so the two cannot drift.
    fn term_wheel(&self, x: f32, y: f32, dy: f32) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let k = f64::from(app.window().scale_factor()).max(0.01);
        self.kp.wheel(
            crate::window::WheelDelta::Lines(wheel_lines(dy)),
            (f64::from(x) * k, f64::from(y) * k),
            false,
        );
    }

    /// One key through the path a real key takes; a key the app leaves to Slint reaches it too,
    /// with the modifiers a real keyboard would have pressed before it.
    fn key(&self, ev: &KeyEvent) -> Result<(), String> {
        let result = crate::window::handle_key(&self.kp, ev);
        if matches!(result, EventResult::Propagate) {
            self.dispatch(slint_key_events(ev))?;
        } else if !ev.pressed {
            // The key was the app's, but its press may have gone to Slint with the modifiers: a
            // release is never forwarded, so Slint is told the modifiers are up here.
            self.dispatch(slint_modifier_releases(ev))?;
        }
        Ok(())
    }

    fn with_ctl<R>(&self, f: impl FnOnce(&mut AppCtl) -> R) -> Result<R, String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        Ok(f(&mut c))
    }

    fn quit(&self, force: bool) -> Result<(), String> {
        let decision = self.with_ctl(|c| c.on_close_requested())?;
        if decision == CloseDecision::Close || force {
            if let Some(app) = self.app.upgrade() {
                let _ = app.hide();
            }
            let _ = slint::quit_event_loop();
        }
        Ok(())
    }

    fn window_op(&self, op: &str) -> Result<(), String> {
        let app = self.app.upgrade().ok_or("the window is gone")?;
        let w = app.window();
        match op {
            "minimise" | "minimize" => w.set_minimized(true),
            "restore" => {
                // The window is restored first without activation; the Slint calls then only bring
                // its own minimised/maximised state in step (winit does nothing for a window that
                // is already restored).
                restore_without_activating(&app)?;
                w.set_minimized(false);
                w.set_maximized(false);
            }
            "maximise" | "maximize" => w.set_maximized(true),
            "fullscreen" => {
                // The same chord the user presses.
                let ev = event_from_chord("F11", true)?;
                self.key(&ev)?;
                self.key(&KeyEvent {
                    pressed: false,
                    ..ev
                })?;
            }
            other => return Err(format!("unknown window op {other:?}")),
        }
        Ok(())
    }

    fn dpi_change(&self, dpi: u32, origin: Option<(i32, i32)>) -> Result<(), String> {
        if dpi == 0 {
            return Err("dpi must be above zero".into());
        }
        let app = self.app.upgrade().ok_or("the window is gone")?;
        let hwnd = crate::window::hwnd_of(&app).ok_or("the window has no handle")?;
        apply_dpi(hwnd, app.window(), dpi, origin, DpiMode::from_env());
        Ok(())
    }
    /// `SlideHold`: the sidebar's slide at an offset, held for a picture (or let go).
    fn slide_hold(&self, ms: Option<f64>) -> Result<(), String> {
        let ctl = self.ctl.upgrade().ok_or("the window is gone")?;
        let mut c = ctl.try_borrow_mut().map_err(|_| "the controller is busy")?;
        c.hold_slide(ms);
        Ok(())
    }

    /// `Snapshot`: Slint's own snapshot first, then `PrintWindow`; the PNG is in device pixels.
    fn snapshot(&self, path: &str, mode: Option<SnapMode>) -> Reply {
        let Some(app) = self.app.upgrade() else {
            return err("the window is gone");
        };
        let via_print = || -> Result<Snap, String> {
            let hwnd = crate::window::hwnd_of(&app).ok_or("the window has no handle")?;
            print_window_snapshot(hwnd)
        };
        let snap = match mode.unwrap_or(SnapMode::Auto) {
            SnapMode::Slint => slint_snapshot(app.window()),
            SnapMode::PrintWindow => via_print(),
            SnapMode::Auto => {
                let first = slint_snapshot(app.window());
                match pick_snapshot(&first) {
                    SnapPick::UseSlint => {
                        log::info!("e2e: snapshot mode auto used the Slint snapshot");
                        first
                    }
                    SnapPick::PrintWindow => {
                        log::info!("e2e: snapshot mode auto fell back to PrintWindow");
                        via_print()
                    }
                }
            }
        };
        let snap = match snap {
            Ok(s) => s,
            Err(m) => return err(m),
        };
        let png = match encode_png(&snap) {
            Ok(p) => p,
            Err(m) => return err(m),
        };
        // sync-ok: e2e only, a screenshot written for the harness
        if let Err(e) = std::fs::write(path, png) {
            return err(format!("cannot write {path}: {e}"));
        }
        let reply = Reply::Png {
            path: path.to_string(),
            w: snap.w,
            h: snap.h,
        };
        // Read now, on the UI thread, straight after the picture: the slide the picture shows (audit V14).
        let slide = self.slide();
        // And the breath it shows: a question mark breathes from full to nothing and back, so a picture
        // of it is only judged against the strength it was drawn at (the indicatorStyles flake, 2026-10-21).
        let breathe = {
            let g = app.global::<crate::MarkMotion>();
            shown_breathe(g.get_still(), g.get_breathe())
        };
        FRAMES.with(|f| {
            if let Ok(mut st) = f.try_borrow_mut() {
                st.snap = Some(snap);
                st.set_snap_slide(slide);
                st.set_snap_breathe(Some(breathe));
            }
        });
        reply
    }

    fn clip_set(&self, text: Option<&str>, png: Option<&str>) -> Result<(), String> {
        if !crate::e2e::global().on() {
            return Err("the real clipboard is never written".into());
        }
        let port = self.clip.as_ref().ok_or("no clipboard port")?;
        if let Some(png) = png {
            return port.set_image_png_b64(png);
        }
        let text = text.ok_or("ClipSet needs text")?;
        port.write_text(text, usize::MAX)
    }
}

impl FrameEnv for GlueCtx {
    fn view(&self, tab: Option<&str>) -> Result<FrameView, String> {
        if tab.is_some() {
            return Err("only the front tab can be read by Latency for now".into());
        }
        let handle = self.kp.front.borrow().clone().ok_or("no tab is in front")?;
        Ok(handle.with_emu_quiet(|e| {
            let text = (0..e.rows() as i32)
                .map(|l| e.row_text(l))
                .collect::<Vec<_>>()
                .join("\n");
            FrameView {
                text,
                cursor_row: e.row_text(e.cursor().0),
            }
        }))
    }

    fn press_down(&self, chord: &str) -> Result<(), String> {
        event_from_chord(chord, true).and_then(|e| self.key(&e))
    }

    fn slide(&self) -> Option<(f32, f32)> {
        let app = self.app.upgrade()?;
        Some((app.get_sidebar_slot_visual(), app.get_sidebar_panel_left()))
    }
}

impl GlueCtx {
    /// `Probe`: a read-only look, answered at once from the UI thread.
    fn probe(&self, name: &str, arg: &serde_json::Value) -> Reply {
        let (Some(app), Some(ctl)) = (self.app.upgrade(), self.ctl.upgrade()) else {
            return err("the window is gone");
        };
        let pctx = crate::e2e_probes::ProbeCtx {
            app: &app,
            ctl: &ctl,
            args: &self.args,
            clip: self.clip.as_ref(),
            front: self.kp.front.borrow().clone(),
        };
        match self.probes.call(name, &pctx, arg) {
            Ok(value) => Reply::Value { value },
            Err(message) => Reply::Err { message },
        }
    }

    /// `Latency`: the key goes in now, the answer comes from the first frame holding `expect`.
    fn latency(
        &self,
        chord: &str,
        expect: &str,
        tab: Option<String>,
        timeout_ms: u64,
        resp: Responder,
    ) {
        let timeout_ms = clamp_timeout_ms(timeout_ms);
        FRAMES.with(|f| {
            if let Ok(mut st) = f.try_borrow_mut() {
                st.rec.start();
            }
        });
        let t0 = Instant::now();
        if let Err(m) = event_from_chord(chord, true).and_then(|e| self.key(&e)) {
            resp.reply(err(m));
            return;
        }
        let seq = FRAMES.with(|f| {
            f.try_borrow_mut()
                .ok()
                .map(|mut st| st.arm_latency(LatencyWatch::new(t0, expect, timeout_ms), tab, resp))
        });
        if let Some(seq) = seq {
            slint::Timer::single_shot(Duration::from_millis(timeout_ms), move || {
                FRAMES.with(|f| {
                    if let Ok(mut st) = f.try_borrow_mut() {
                        st.latency_timed_out(seq);
                    }
                });
            });
        }
        self.redraw();
    }

    /// `KeyAtPrompt`: replies at once; the render hook injects the key when the prompt shows.
    fn key_at_prompt(&self, chord: &str, prompt_suffix: &str) -> Result<(), String> {
        let ev = event_from_chord(chord, true)?;
        FRAMES.with(|f| {
            let mut st = f.try_borrow_mut().map_err(|_| "the frame state is busy")?;
            st.rec.start();
            st.marks = (None, None);
            st.prompt = Some((PromptWatch::new(prompt_suffix, ev.text), chord.to_string()));
            Ok::<(), &str>(())
        })?;
        self.redraw();
        Ok(())
    }

    /// Asks for a frame, so a watcher on an idle window gets one to look at.
    fn redraw(&self) {
        if let Some(app) = self.app.upgrade() {
            app.window().request_redraw();
        }
    }
}

/// `Pixel`: a pixel of the last snapshot.
fn pixel_reply(x: u32, y: u32) -> Reply {
    FRAMES.with(|f| match f.try_borrow() {
        Ok(st) => match st.snap.as_ref() {
            None => err("no snapshot has been taken"),
            Some(s) => match s.px(x, y) {
                Some(rgba) => Reply::Pixel { rgba },
                None => err(format!(
                    "({x}, {y}) is outside the {}x{} snapshot",
                    s.w, s.h
                )),
            },
        },
        Err(_) => err("the frame state is busy"),
    })
}

/// The software renderer has no rendering notifier: the series is the window's redraw requests from now on.
pub fn series_from_redraws() {
    FRAMES.with(|f| {
        if let Ok(mut st) = f.try_borrow_mut() {
            st.rec.use_redraw_series();
        }
    });
}

/// Called for every redraw request of the window (software renderer only; a no-op otherwise).
pub fn on_redraw() {
    if !crate::e2e::global().on() {
        return;
    }
    let now = Instant::now();
    let age = crate::startup_trace::since_process_start().as_secs_f64() * 1000.0;
    let env = FRAMES.with(|f| f.try_borrow().ok().and_then(|st| st.env.clone()));
    let slide = env.and_then(|e| e.slide());
    FRAMES.with(|f| {
        if let Ok(mut st) = f.try_borrow_mut() {
            st.rec.redraw_sample(now, age, slide);
        }
    });
}

/// `frames.series`: every presented frame since the last `Frames` reset (or since the start), as
/// `[age_ms, gap_ms, render_ms, slot, panel_left]` rows, and the process age now.
pub fn frame_series() -> serde_json::Value {
    let (frames, snap_slide, snap_breathe) = FRAMES.with(|f| match f.try_borrow() {
        Ok(st) => (
            st.rec.series_json(),
            st.snap_slide_json(),
            st.snap_breathe_json(),
        ),
        Err(_) => (
            serde_json::json!([]),
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
    });
    serde_json::json!({
        "frames": frames,
        "snap_slide": snap_slide,
        "snap_breathe": snap_breathe,
        "now_age_ms": crate::startup_trace::since_process_start().as_secs_f64() * 1000.0,
    })
}

/// `Frames`.
fn frames_reply(reset: bool) -> Reply {
    FRAMES.with(|f| match f.try_borrow_mut() {
        Ok(mut st) => st.frames_request(reset),
        Err(_) => err("the frame state is busy"),
    })
}

/// Holds the UI thread for `ms`, after the reply went out (the client's call returns at once and
/// the app is then blocked, as a stall would).
pub fn block_ui(ms: u64, resp: Responder, sleep: impl FnOnce(Duration)) {
    resp.reply(Reply::Ok);
    sleep(Duration::from_millis(ms)); // sync-ok: e2e only, holds the UI thread on purpose
}

// ---- the DPI change (Task 9) ---------------------------------------------------------------

/// 96 dpi is 100 percent.
pub fn dpi_to_scale(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// The rectangle Windows would suggest in `WM_DPICHANGED` for a window of `logical` size at
/// `origin`: left, top, right, bottom in device px, the size being `round(logical * dpi / 96)`.
pub fn suggested_rect(origin: (i32, i32), logical: (f32, f32), dpi: u32) -> (i32, i32, i32, i32) {
    let s = dpi_to_scale(dpi);
    let w = (logical.0 * s).round() as i32;
    let h = (logical.1 * s).round() as i32;
    (origin.0, origin.1, origin.0 + w, origin.1 + h)
}

/// How a DPI change reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiMode {
    /// A real `WM_DPICHANGED` through the window procedure (the default).
    Message,
    /// The fallback: the `ScaleFactorChanged` event, then the new size.
    Event,
}

impl DpiMode {
    /// `FLARE_E2E_DPI_MODE=event` picks the fallback; anything else (or absent) the message.
    pub fn from_env_value(v: Option<&str>) -> DpiMode {
        match v {
            Some(s) if s.trim().eq_ignore_ascii_case("event") => DpiMode::Event,
            _ => DpiMode::Message,
        }
    }

    pub fn from_env() -> DpiMode {
        let name = ["FLARE_E2E", "_DPI_MODE"].concat();
        DpiMode::from_env_value(crate::e2e::global().env(&name).as_deref())
    }
}

/// What the event fallback needs from a window; a fake records the calls in the test.
pub trait DpiSeam {
    fn scale_factor_changed(&mut self, scale_factor: f32);
    fn set_physical_size(&mut self, w: u32, h: u32);
}

/// A `&slint::Window` as a `DpiSeam`.
struct WindowRef<'a>(&'a slint::Window);

impl DpiSeam for WindowRef<'_> {
    fn scale_factor_changed(&mut self, scale_factor: f32) {
        self.0
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
    }

    fn set_physical_size(&mut self, w: u32, h: u32) {
        self.0.set_size(slint::PhysicalSize::new(w, h));
    }
}

/// The event fallback: the same pair winit would send (the scale, then the resize to the suggested
/// size).
pub fn apply_dpi_event(seam: &mut dyn DpiSeam, logical: (f32, f32), dpi: u32) {
    let (l, t, r, b) = suggested_rect((0, 0), logical, dpi);
    seam.scale_factor_changed(dpi_to_scale(dpi));
    seam.set_physical_size((r - l) as u32, (b - t) as u32);
}

/// Changes the window's scaling as a monitor move would. `origin` is the new top-left in device
/// px, else the window's own. Runs on the UI thread; the reply is sent after it returns, and the
/// client then waits for the `app.state.scale` probe (the glue never claims success itself).
pub fn apply_dpi(
    hwnd: isize,
    window: &slint::Window,
    dpi: u32,
    origin: Option<(i32, i32)>,
    mode: DpiMode,
) {
    let scale = window.scale_factor().max(0.01);
    let size = window.size();
    let logical = (size.width as f32 / scale, size.height as f32 / scale);
    match mode {
        #[cfg(not(windows))]
        DpiMode::Message => {
            // macOS: no WM_DPICHANGED; the event path stands in (the Mac E2E, port plan "Ongoing").
            let _ = (hwnd, origin);
            apply_dpi_event(&mut WindowRef(window), logical, dpi)
        }
        #[cfg(windows)]
        DpiMode::Message => {
            use windows_sys::Win32::Foundation::RECT;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowRect, SendMessageW, WM_DPICHANGED,
            };
            let h = hwnd as windows_sys::Win32::Foundation::HWND;
            let at = origin.unwrap_or_else(|| {
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                // SAFETY: `h` is this process's window and `r` is a valid out pointer.
                unsafe { GetWindowRect(h, &mut r) };
                (r.left, r.top)
            });
            let (left, top, right, bottom) = suggested_rect(at, logical, dpi);
            let rect = RECT {
                left,
                top,
                right,
                bottom,
            };
            let wparam = (dpi as usize) | ((dpi as usize) << 16);
            // SAFETY: sent from the UI thread to its own window; `rect` outlives the synchronous call.
            unsafe {
                SendMessageW(h, WM_DPICHANGED, wparam, &rect as *const RECT as isize);
            }
        }
        DpiMode::Event => apply_dpi_event(&mut WindowRef(window), logical, dpi),
    }
}
/// The one `match` over every request. Tasks 8 to 11 each own the arms named in their title.
fn handle(ctx: &GlueCtx, req: Request, resp: Responder) {
    let result: Result<(), String> = match req {
        Request::Key { chord, down } => event_from_chord(&chord, down).and_then(|e| ctx.key(&e)),
        Request::KeyPress { chord } => event_from_chord(&chord, true)
            .and_then(|e| ctx.key(&e))
            .and_then(|()| event_from_chord(&chord, false))
            .and_then(|e| ctx.key(&e)),
        Request::Text { text } => text.chars().try_for_each(|c| {
            ctx.key(&event_from_char(c, true))?;
            ctx.key(&event_from_char(c, false))
        }),
        Request::Pointer { x, y, action } => {
            ctx.dispatch(pointer_events(x, y, &action)).map(|()| {
                if let PointerAction::Wheel { dy, .. } = action {
                    ctx.term_wheel(x, y, dy);
                }
            })
        }
        Request::Drag {
            from,
            to,
            steps,
            button,
        } => ctx.dispatch(drag_events(from, to, steps, button)),
        Request::HoverFiles => ctx.with_ctl(|c| hover_files(c)),
        Request::DropFiles { paths, x, y } => ctx.with_ctl(|c| drop_files(c, &paths, (x, y))),
        Request::Resize { w, h } => ctx
            .app
            .upgrade()
            .ok_or_else(|| "the window is gone".to_string())
            .map(|a| a.window().set_size(slint::LogicalSize::new(w, h))),
        Request::Focus { on } => {
            crate::window::handle_focus(&ctx.kp, on);
            ctx.dispatch(vec![WindowEvent::WindowActiveChanged(on)])
        }
        Request::Window { op } => ctx.window_op(&op),
        Request::ClipSet { text, png_b64 } => ctx.clip_set(text.as_deref(), png_b64.as_deref()),
        Request::Slow { name, ms } => {
            // sync-ok: e2e only, a work item that takes `ms` so the work timer reports it
            crate::diag_ctl::timed(&name, || std::thread::sleep(Duration::from_millis(ms)));
            Ok(())
        }
        Request::BlockUi { ms } => {
            // sync-ok: e2e only, holds the UI thread on purpose (diagLog)
            block_ui(ms, resp, std::thread::sleep);
            return;
        }
        Request::PanicWorker => {
            let _ = std::thread::Builder::new()
                .name("e2e-panic".into())
                .spawn(|| panic!("e2e: worker panic"));
            Ok(())
        }
        Request::Quit { force } => ctx.quit(force),
        // Tasks 9 to 11 replace these.
        Request::Hello => Err("Hello is answered by the server".into()),
        Request::DpiChange { dpi, x, y } => ctx.dpi_change(dpi, x.zip(y)),
        Request::Snapshot { path, mode } => {
            resp.reply(ctx.snapshot(&path, mode));
            return;
        }
        Request::Pixel { x, y } => {
            resp.reply(pixel_reply(x, y));
            return;
        }
        Request::Frames { reset } => {
            resp.reply(frames_reply(reset));
            return;
        }
        Request::SlideHold { ms } => ctx.slide_hold(ms),
        Request::Latency {
            chord,
            expect,
            tab,
            timeout_ms,
        } => {
            ctx.latency(&chord, &expect, tab, timeout_ms, resp);
            return;
        }
        Request::KeyAtPrompt {
            chord,
            prompt_suffix,
        } => ctx.key_at_prompt(&chord, &prompt_suffix),
        Request::Probe { name, arg } => {
            resp.reply(ctx.probe(&name, &arg));
            return;
        }
        Request::DictLine { line } => crate::dictation_ctl::e2e_line(&line),
        Request::DictRoute { text, front } => crate::dictation_ctl::e2e_route(&text, front),
        Request::DictRouted { result } => crate::dictation_ctl::e2e_routed(&result),
        Request::WebScript { js } => ctx.web_script(&js),
        Request::WebUserInput => ctx.web_user_input(),
        Request::WebZoom { factor } => ctx.web_zoom(factor),
        Request::ToastClick { tab } => ctx.toast_click(&tab),
    };
    resp.reply(match result {
        Ok(()) => Reply::Ok,
        Err(m) => err(m),
    });
}

/// Starts the hook for an `--e2e` run: parks the window, starts the pipe server (Task 7), registers
/// the UI handler and the probe registry. The caller (window.rs) runs this only inside its
/// `if args.e2e` block.
pub fn install(
    app: &AppWindow,
    ctl: &Rc<RefCell<AppCtl>>,
    args: &Args,
    kp: &Rc<KeyPath>,
    clip: Option<Arc<dyn ClipboardPort>>,
) {
    // Parking is window.rs's job (`park_e2e_window_when_ready`): the native window does not exist
    // yet when this runs, so no look for its handle is made here.
    let appdata = std::env::var("APPDATA").ok();
    let profile = crate::startup::profile_dir(args.user_data_dir.as_deref(), appdata.as_deref());
    match crate::e2e_hook::start(&profile) {
        Ok(server) => {
            log::info!("e2e: the test hook listens on {}", server.pipe);
            HOOK.with(|h| *h.borrow_mut() = Some(server));
        }
        Err(e) => log::warn!("e2e: the test hook did not start: {e:?}"),
    }
    let mut probes = crate::e2e_probes::ProbeRegistry::new();
    crate::e2e_probes::register_all(&mut probes);
    let ctx = Rc::new(GlueCtx {
        app: app.as_weak(),
        ctl: Rc::downgrade(ctl),
        kp: kp.clone(),
        clip,
        args: args.clone(),
        probes,
    });
    let env: Rc<dyn FrameEnv> = ctx.clone();
    FRAMES.with(|f| f.borrow_mut().env = Some(env));
    crate::e2e_hook::register_ui_handler(Rc::new(move |req, resp| handle(&ctx, req, resp)));
}

/// macOS: the Mac E2E's own capture (port plan "Ongoing"). Refused until then.
#[cfg(not(windows))]
fn print_window_snapshot(_hwnd: isize) -> Result<Snap, String> {
    Err("the window snapshot is not built on this platform yet".into())
}

/// macOS: the Mac E2E (port plan "Ongoing"). Refused until then.
#[cfg(not(windows))]
fn restore_without_activating(_app: &AppWindow) -> Result<(), String> {
    Err("restoring without activation is not built on this platform yet".into())
}

#[cfg(test)]
mod tests {
    // L59: pipe input must not panic the app or exhaust memory.
    #[test]
    fn a_huge_latency_timeout_does_not_overflow_the_deadline() {
        let w = LatencyWatch::new(Instant::now(), "x", u64::MAX);
        assert_eq!(w.frame(Instant::now(), "no"), LatencyStep::Waiting);
        assert_eq!(clamp_timeout_ms(u64::MAX), MAX_TIMEOUT_MS);
        assert_eq!(clamp_timeout_ms(500), 500);
    }

    #[test]
    fn a_drag_with_absurd_steps_stays_bounded() {
        let drag = drag_events((0.0, 0.0), (10.0, 0.0), u32::MAX, Button::Left);
        assert_eq!(drag.len(), 2 + MAX_DRAG_STEPS as usize + 1);
    }

    // L58: a restore from the pipe must not take the foreground.
    #[cfg(windows)]
    #[test]
    fn a_restore_shows_the_window_without_activating_it() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, WINDOWPLACEMENT};
        // SAFETY: WINDOWPLACEMENT is plain data.
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.showCmd = 3; // SW_SHOWMAXIMIZED
        let r = restore_placement(p);
        assert_eq!(r.showCmd, SW_SHOWNOACTIVATE as u32);
    }

    // Task 23: a collapsed sidebar is revealed by the cursor, which the real window hears from winit
    // (`CursorMoved`) and an injected Slint pointer event never reaches.
    #[test]
    fn a_pointer_move_over_a_collapsed_sidebar_is_told_to_the_controller() {
        let at = LogicalPosition::new(2.0, 400.0);
        assert_eq!(
            cursor_forward(&WindowEvent::PointerMoved { position: at }, true),
            Some(Some((2.0, 400.0)))
        );
        assert_eq!(
            cursor_forward(&WindowEvent::PointerExited, true),
            Some(None)
        );
    }

    #[test]
    fn a_pointer_move_is_not_forwarded_while_the_sidebar_is_pinned_or_absent() {
        let at = LogicalPosition::new(2.0, 400.0);
        assert_eq!(
            cursor_forward(&WindowEvent::PointerMoved { position: at }, false),
            None
        );
        assert_eq!(cursor_forward(&WindowEvent::PointerExited, false), None);
    }

    #[test]
    fn an_injected_wheel_keeps_what_is_less_than_a_notch() {
        use crate::live::WheelAccum;
        assert_eq!(wheel_lines(0.0), 0.0);
        assert_eq!(wheel_lines(120.0), 3.0);
        assert_eq!(wheel_lines(-120.0), -3.0);
        assert_eq!(wheel_lines(20_000.0), 500.0);
        assert_eq!(wheel_lines(1.0e9), 4096.0);
        // 1 px used to count as a whole notch; now it adds up, as a touchpad's fractions do.
        let mut acc = WheelAccum::new();
        assert_eq!(acc.add_lines(wheel_lines(1.0)), 0);
        let total: i32 = (0..4).map(|_| acc.add_lines(wheel_lines(10.0))).sum();
        assert_eq!(total, 1, "four 10 px turns are one notch");
    }

    #[test]
    fn only_moves_and_exits_are_forwarded() {
        let at = LogicalPosition::new(2.0, 400.0);
        let press = WindowEvent::PointerPressed {
            position: at,
            button: PointerEventButton::Left,
        };
        assert_eq!(cursor_forward(&press, true), None);
    }
    use super::*;
    use crate::keymap::key_event_from_parts;
    use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn named(n: slint::winit_030::winit::keyboard::NamedKey) -> Key {
        Key::Named(n)
    }

    /// (chord, physical, logical, text, modifiers) as winit would report them.
    fn table() -> Vec<(
        &'static str,
        KeyCode,
        Key,
        Option<&'static str>,
        ModifiersState,
    )> {
        use slint::winit_030::winit::keyboard::NamedKey as N;
        let none = ModifiersState::empty();
        let ctrl = ModifiersState::CONTROL;
        let shift = ModifiersState::SHIFT;
        let alt = ModifiersState::ALT;
        vec![
            ("Ctrl+T", KeyCode::KeyT, ch("t"), Some("\u{14}"), ctrl),
            ("Ctrl+t", KeyCode::KeyT, ch("t"), Some("\u{14}"), ctrl),
            (
                "Ctrl+Shift+KeyC",
                KeyCode::KeyC,
                ch("C"),
                Some("\u{3}"),
                ctrl | shift,
            ),
            ("Shift+Enter", KeyCode::Enter, named(N::Enter), None, shift),
            ("Alt+b", KeyCode::KeyB, ch("b"), Some("b"), alt),
            ("F11", KeyCode::F11, named(N::F11), None, none),
            ("Ctrl+,", KeyCode::Comma, ch(","), Some(","), ctrl),
            ("Ctrl+Tab", KeyCode::Tab, named(N::Tab), None, ctrl),
            ("ArrowUp", KeyCode::ArrowUp, named(N::ArrowUp), None, none),
            ("Ctrl+Alt+q", KeyCode::KeyQ, ch("q"), Some("q"), ctrl | alt),
            ("?", KeyCode::Slash, ch("?"), Some("?"), shift),
            ("t", KeyCode::KeyT, ch("t"), Some("t"), none),
            ("Shift+t", KeyCode::KeyT, ch("T"), Some("T"), shift),
            ("Digit1", KeyCode::Digit1, ch("1"), Some("1"), none),
            ("Shift+Digit1", KeyCode::Digit1, ch("!"), Some("!"), shift),
            ("Space", KeyCode::Space, named(N::Space), Some(" "), none),
            ("Escape", KeyCode::Escape, named(N::Escape), None, none),
            (
                "Backspace",
                KeyCode::Backspace,
                named(N::Backspace),
                None,
                none,
            ),
            ("Ctrl+Delete", KeyCode::Delete, named(N::Delete), None, ctrl),
            ("PageUp", KeyCode::PageUp, named(N::PageUp), None, none),
            ("Home", KeyCode::Home, named(N::Home), None, none),
            ("End", KeyCode::End, named(N::End), None, none),
            ("Ctrl+.", KeyCode::Period, ch("."), Some("."), ctrl),
            ("Ctrl+/", KeyCode::Slash, ch("/"), Some("/"), ctrl),
            ("Ctrl+`", KeyCode::Backquote, ch("`"), Some("`"), ctrl),
            ("Numpad0", KeyCode::Numpad0, ch("0"), Some("0"), none),
            ("F5", KeyCode::F5, named(N::F5), None, none),
        ]
    }

    #[test]
    fn a_chord_builds_the_event_keymap_would_build() {
        let cases = table();
        assert!(cases.len() >= 25, "{} chords", cases.len());
        for (chord, code, logical, text, mods) in cases {
            for down in [true, false] {
                let want =
                    key_event_from_parts(down, &PhysicalKey::Code(code), &logical, text, mods);
                let got = event_from_chord(chord, down).unwrap_or_else(|e| panic!("{chord}: {e}"));
                assert_eq!(got, want, "chord {chord:?} down={down}");
            }
        }
    }

    #[test]
    fn a_modifier_key_alone_is_a_key_event_so_a_held_ctrl_can_be_let_go() {
        use slint::winit_030::winit::keyboard::NamedKey as N;
        for (chord, code, named, mods) in [
            (
                "ControlLeft",
                KeyCode::ControlLeft,
                N::Control,
                ModifiersState::empty(),
            ),
            (
                "ShiftLeft",
                KeyCode::ShiftLeft,
                N::Shift,
                ModifiersState::empty(),
            ),
            ("AltLeft", KeyCode::AltLeft, N::Alt, ModifiersState::empty()),
            // Right Alt alone is the dictation key (M8): a scenario holds it as a key-down and a key-up.
            (
                "AltRight",
                KeyCode::AltRight,
                N::Alt,
                ModifiersState::empty(),
            ),
        ] {
            for down in [true, false] {
                let want = key_event_from_parts(
                    down,
                    &PhysicalKey::Code(code),
                    &Key::Named(named),
                    None,
                    mods,
                );
                let got = event_from_chord(chord, down).unwrap_or_else(|e| panic!("{chord}: {e}"));
                assert_eq!(got, want, "chord {chord:?} down={down}");
            }
        }
    }

    #[test]
    fn a_logical_key_override_keeps_the_physical_key() {
        let got = event_from_chord("Ctrl+KeyC=\u{441}", true).expect("parses");
        assert_eq!(got.code, "KeyC");
        assert_eq!(got.key, "\u{441}");
        assert!(got.ctrl);
        assert_eq!(got.text, None, "Ctrl without Alt drops the text");
        let eq = event_from_chord("Ctrl+=", true).expect("the equals key");
        assert_eq!((eq.code.as_str(), eq.key.as_str()), ("Equal", "="));
        let plus = event_from_chord("Ctrl++", true).expect("the plus key");
        assert_eq!(plus.key, "+");
    }

    #[test]
    fn a_bad_chord_is_an_error_message() {
        for bad in ["Ctrl+", "Foo+T", "", "Ctrl+Nope", "Ctrl+KeyC="] {
            assert!(event_from_chord(bad, true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn text_is_a_key_per_character_and_a_newline_is_enter() {
        let a = event_from_char('a', true);
        assert_eq!((a.code.as_str(), a.text.as_deref()), ("KeyA", Some("a")));
        let big = event_from_char('A', true);
        assert!(big.shift && big.key == "A");
        let enter = event_from_char('\n', true);
        assert_eq!((enter.key.as_str(), enter.text), ("Enter", None));
        let accent = event_from_char('\u{e9}', true);
        assert_eq!(accent.text.as_deref(), Some("\u{e9}"));
    }

    #[test]
    fn a_click_is_move_press_release_at_the_logical_position() {
        let evs = pointer_events(
            10.5,
            20.0,
            &PointerAction::Click {
                button: Button::Left,
                count: 2,
            },
        );
        assert_eq!(evs.len(), 6);
        assert!(matches!(
            evs[1],
            WindowEvent::PointerPressed {
                button: PointerEventButton::Left,
                ..
            }
        ));
        if let WindowEvent::PointerMoved { position } = &evs[0] {
            assert_eq!((position.x, position.y), (10.5, 20.0));
        } else {
            panic!("the first event is a move");
        }
        let drag = drag_events((0.0, 0.0), (10.0, 0.0), 5, Button::Left);
        assert_eq!(drag.len(), 2 + 5 + 1);
        assert!(matches!(
            drag.last(),
            Some(WindowEvent::PointerReleased { .. })
        ));
    }

    #[derive(Default)]
    struct Fake(Vec<String>);

    impl DropTarget for Fake {
        fn hovered(&mut self) {
            self.0.push("hover".into());
        }
        fn dropped(&mut self, path: PathBuf, at: (f32, f32)) {
            self.0.push(format!("drop {} {:?}", path.display(), at));
        }
    }

    #[test]
    fn drop_calls_hover_then_drop() {
        let mut f = Fake::default();
        drop_files(&mut f, &["C:/a.txt".into(), "C:/b.txt".into()], (5.0, 6.0));
        assert_eq!(
            f.0,
            vec![
                "hover",
                "drop C:/a.txt (5.0, 6.0)",
                "drop C:/b.txt (5.0, 6.0)"
            ]
        );
    }

    #[test]
    fn a_hover_alone_never_drops() {
        let mut f = Fake::default();
        hover_files(&mut f);
        assert_eq!(f.0, vec!["hover"]);
    }

    #[test]
    fn a_drag_in_flight_makes_the_controller_hear_the_cursor() {
        let at = LogicalPosition::new(40.0, 10.0);
        let mv = WindowEvent::PointerMoved { position: at };
        assert!(cursor_listening(false, true));
        assert!(cursor_listening(true, false));
        assert!(!cursor_listening(false, false));
        assert_eq!(
            cursor_forward(&mv, cursor_listening(false, true)),
            Some(Some((40.0, 10.0)))
        );
    }

    #[test]
    fn block_ui_replies_before_it_blocks() {
        let (resp, rx) = crate::e2e_hook::responder();
        let order = std::cell::RefCell::new(Vec::new());
        block_ui(40, resp, |d| {
            order.borrow_mut().push(format!("sleep {}", d.as_millis()));
            // By now the reply is already waiting.
            assert_eq!(rx.try_recv().ok(), Some(Reply::Ok));
        });
        assert_eq!(order.into_inner(), vec!["sleep 40"]);
    }

    #[test]
    fn install_does_not_look_for_a_handle_the_window_has_not_got_yet() {
        // The native window exists only once the event loop runs, so a look for it inside `install`
        // always failed ("the window has no handle, it is not parked"). window.rs parks it by timer.
        let src = &include_str!("e2e_glue.rs").replace("\r\n", "\n");
        let from = src.find("pub fn install(").expect("install exists");
        let to = src[from..].find("\n}\n").expect("install ends") + from;
        let body = &src[from..to];
        assert!(
            !body.contains("hwnd_of"),
            "install must not look for the handle"
        );
        let win = include_str!("window.rs");
        assert!(win.contains("park_e2e_window_when_ready(&app)"));
        assert!(win.contains("flare_win::window_park::park(h)"));
    }

    #[test]
    fn named_keys_reach_slint_as_its_own_characters() {
        let esc = event_from_chord("Escape", true).expect("parses");
        let t = slint_text_for(&esc).expect("a text");
        assert_eq!(
            t.as_str(),
            String::from(char::from(slint::platform::Key::Escape))
        );
        assert!(slint_text_for(&event_from_chord("F11", true).expect("parses")).is_none());
    }

    #[test]
    fn a_chord_with_modifiers_reaches_slint_as_modifier_presses_around_the_key() {
        use slint::platform::Key;
        let kind = |e: &WindowEvent| match e {
            WindowEvent::KeyPressed { text } => format!("down {:?}", text.as_str()),
            WindowEvent::KeyReleased { text } => format!("up {:?}", text.as_str()),
            _ => "other".to_string(),
        };
        let shift = format!("{:?}", String::from(char::from(Key::Shift)));
        let ctrl = format!("{:?}", String::from(char::from(Key::Control)));
        let left = format!("{:?}", String::from(char::from(Key::LeftArrow)));
        let down: Vec<String> =
            slint_key_events(&event_from_chord("Shift+ArrowLeft", true).unwrap())
                .iter()
                .map(kind)
                .collect();
        assert_eq!(down, vec![format!("down {shift}"), format!("down {left}")]);
        let up: Vec<String> =
            slint_key_events(&event_from_chord("Shift+ArrowLeft", false).unwrap())
                .iter()
                .map(kind)
                .collect();
        assert_eq!(up, vec![format!("up {left}"), format!("up {shift}")]);
        // Ctrl+A is the character with Control held, so a text field selects all.
        let sel: Vec<String> = slint_key_events(&event_from_chord("Ctrl+KeyA", true).unwrap())
            .iter()
            .map(kind)
            .collect();
        assert_eq!(sel, vec![format!("down {ctrl}"), "down \"a\"".to_string()]);
        // A plain key carries no modifier, and a key Slint has no text for sends nothing.
        assert_eq!(
            slint_key_events(&event_from_chord("ArrowLeft", true).unwrap()).len(),
            1
        );
        // A release the app took still lets go of the modifiers Slint saw pressed.
        let rel = slint_modifier_releases(&event_from_chord("Ctrl+Shift+KeyA", false).unwrap());
        assert_eq!(rel.len(), 2);
        assert!(
            rel.iter()
                .all(|e| matches!(e, WindowEvent::KeyReleased { .. }))
        );
        assert!(slint_modifier_releases(&event_from_chord("KeyA", false).unwrap()).is_empty());
        assert!(slint_key_events(&event_from_chord("F11", true).unwrap()).is_empty());
    }

    #[test]
    fn a_click_reaches_the_slint_component_at_every_scale() {
        use crate::ui_harness::{Harness, SCALES};
        use crate::{StripGeom, TabStripPreview};
        use slint::ModelRc;
        for s in SCALES {
            let h = Harness::new(s, 400, 36, || {
                let c = TabStripPreview::new().expect("the strip builds");
                c.set_rows(ModelRc::default());
                c.set_geom(StripGeom {
                    more_x: 0,
                    more_w: 0,
                    plus_x: 100,
                    plus_w: 28,
                    more_count: 0,
                    dots: ModelRc::default(),
                });
                c.set_strip_x(0.0);
                c.set_new_tab_title("New tab".into());
                c
            });
            let news = Rc::new(RefCell::new(0));
            let n = news.clone();
            h.comp.on_new_tab(move || *n.borrow_mut() += 1);
            // The strip's geometry is in device pixels: the button's centre is 114 of them.
            for e in pointer_events(
                114.0 / s,
                18.0,
                &PointerAction::Click {
                    button: Button::Left,
                    count: 1,
                },
            ) {
                h.comp.window().dispatch_event(e);
            }
            assert_eq!(*news.borrow(), 1, "scale {s}");
        }
    }

    #[test]
    fn an_unconsumed_key_with_text_reaches_a_slint_text_field() {
        use crate::FramePreview;
        use crate::ui_harness::Harness;
        let h = Harness::new(1.0, 1000, 604, || {
            FramePreview::new().expect("the preview builds")
        });
        h.advance(0);
        h.comp.invoke_focus_field();
        h.advance(0);
        let send = |ev: KeyEvent| {
            if let Some(text) = slint_text_for(&ev) {
                let e = if ev.pressed {
                    WindowEvent::KeyPressed { text }
                } else {
                    WindowEvent::KeyReleased { text }
                };
                h.comp.window().dispatch_event(e);
            }
        };
        for c in "colour".chars() {
            send(event_from_char(c, true));
            send(event_from_char(c, false));
        }
        for down in [true, false] {
            send(event_from_chord("Escape", down).expect("parses"));
        }
        assert_eq!(
            h.comp.get_escape_unhandled_count(),
            0,
            "a field with text claims Escape"
        );
        assert_eq!(h.comp.get_last_key(), "find:Escape");
    }

    #[test]
    fn dpi_96_to_288_is_the_eight_scales() {
        let want = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
        for (dpi, w) in [96, 120, 144, 168, 192, 216, 240, 288]
            .into_iter()
            .zip(want)
        {
            assert!((dpi_to_scale(dpi) - w).abs() < 1e-6, "dpi {dpi}");
        }
    }

    #[test]
    fn the_suggested_rect_keeps_the_logical_size_and_the_origin() {
        for dpi in [96u32, 120, 144, 168, 192, 216, 240, 288] {
            let s = dpi_to_scale(dpi);
            let (l, t, r, b) = suggested_rect((-1920, 40), (1000.0, 640.0), dpi);
            assert_eq!((l, t), (-1920, 40), "origin at {dpi}");
            assert_eq!(r - l, (1000.0 * s).round() as i32, "width at {dpi}");
            assert_eq!(b - t, (640.0 * s).round() as i32, "height at {dpi}");
        }
    }

    #[derive(Default)]
    struct FakeWindow {
        calls: Vec<String>,
    }

    impl DpiSeam for FakeWindow {
        fn scale_factor_changed(&mut self, scale_factor: f32) {
            self.calls.push(format!("scale {scale_factor}"));
        }

        fn set_physical_size(&mut self, w: u32, h: u32) {
            self.calls.push(format!("size {w}x{h}"));
        }
    }

    #[test]
    fn the_event_fallback_sends_the_same_pair_winit_would() {
        let mut w = FakeWindow::default();
        apply_dpi_event(&mut w, (800.0, 600.0), 144);
        assert_eq!(w.calls, vec!["scale 1.5", "size 1200x900"]);
    }

    #[test]
    fn the_default_mode_is_the_message() {
        assert_eq!(DpiMode::from_env_value(None), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("")), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("nonsense")), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("event")), DpiMode::Event);
        assert_eq!(DpiMode::from_env_value(Some("EVENT")), DpiMode::Event);
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::e2e_hook::responder;
    use std::cell::Cell;

    struct FakeEnv {
        view: RefCell<Result<FrameView, String>>,
        pressed: Cell<u32>,
        slide: Cell<Option<(f32, f32)>>,
    }

    impl FakeEnv {
        fn showing(text: &str, row: &str) -> Self {
            Self {
                view: RefCell::new(Ok(FrameView {
                    text: text.into(),
                    cursor_row: row.into(),
                })),
                pressed: Cell::new(0),
                slide: Cell::new(None),
            }
        }

        fn show(&self, text: &str, row: &str) {
            *self.view.borrow_mut() = Ok(FrameView {
                text: text.into(),
                cursor_row: row.into(),
            });
        }
    }

    impl FrameEnv for FakeEnv {
        fn view(&self, _tab: Option<&str>) -> Result<FrameView, String> {
            self.view.borrow().clone()
        }
        fn press_down(&self, _chord: &str) -> Result<(), String> {
            self.pressed.set(self.pressed.get() + 1);
            Ok(())
        }
        fn slide(&self) -> Option<(f32, f32)> {
            self.slide.get()
        }
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_see_through_snapshot_is_written_as_straight_alpha() {
        // Slint hands back the window's pixels premultiplied (what the window shows over the desktop). The
        // PNG says straight RGBA, so a ground of (23,23,23) at alpha 153 must read (23,23,23,153), not (14,14,14,153).
        let mut px = vec![14, 14, 14, 153, 5, 6, 7, 0, 10, 20, 30, 255, 100, 0, 0, 128];
        unpremultiply(&mut px);
        assert_eq!(&px[0..4], &[23, 23, 23, 153]);
        assert_eq!(&px[4..8], &[0, 0, 0, 0], "a clear pixel stays clear");
        assert_eq!(
            &px[8..12],
            &[10, 20, 30, 255],
            "an opaque pixel is untouched"
        );
        assert_eq!(&px[12..16], &[199, 0, 0, 128]);
    }

    #[test]
    fn a_blank_snapshot_falls_back_to_print_window() {
        let blank = Snap {
            w: 2,
            h: 2,
            rgba: vec![7; 16],
        };
        assert_eq!(pick_snapshot(&Ok(blank)), SnapPick::PrintWindow);
        assert_eq!(pick_snapshot(&Err("no".into())), SnapPick::PrintWindow);
        let empty = Snap {
            w: 0,
            h: 0,
            rgba: vec![],
        };
        assert_eq!(pick_snapshot(&Ok(empty)), SnapPick::PrintWindow);
        let mut varied = vec![7u8; 16];
        varied[12] = 9;
        let ok = Snap {
            w: 2,
            h: 2,
            rgba: varied,
        };
        assert_eq!(pick_snapshot(&Ok(ok)), SnapPick::UseSlint);
    }

    slint::slint! {
        export component SnapWin inherits Window {
            width: 200px;
            height: 100px;
            background: #102030;
            Rectangle { x: 10px; y: 10px; width: 40px; height: 20px; background: #ff0000; }
        }
    }

    #[test]
    fn a_snapshot_is_device_pixels() {
        for s in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let h = crate::ui_harness::Harness::new(s, 200, 100, || SnapWin::new().unwrap());
            let frame = h.render();
            let snap = Snap {
                w: frame.w,
                h: frame.h,
                rgba: frame.rgba.clone(),
            };
            let bytes = encode_png(&snap).expect("encodes");
            let dec = png::Decoder::new(std::io::Cursor::new(bytes));
            let reader = dec.read_info().expect("a png");
            let info = reader.info();
            assert_eq!(info.width, (200.0 * s).round() as u32, "width at {s}");
            assert_eq!(info.height, (100.0 * s).round() as u32, "height at {s}");
            assert!(!snap.is_blank(), "something is drawn at {s}");
        }
    }

    #[test]
    fn the_series_keeps_every_frame_with_its_gap_its_render_time_and_the_slide() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        let mut st = FrameState::new();
        for i in 0..3u64 {
            env.slide
                .set(Some((220.0 - 70.0 * i as f32, -10.0 * i as f32)));
            st.rec.before(at(base, i * 16));
            st.after(at(base, i * 16 + 2), 1000.0 + (i * 16) as f64, &env);
        }
        let s = st.rec.series();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].gap_ms, None, "the first frame has no gap behind it");
        assert!((s[1].gap_ms.unwrap() - 16.0).abs() < 0.5);
        assert!((s[2].render_ms.unwrap() - 2.0).abs() < 0.5);
        assert_eq!(s[1].age_ms, 1016.0);
        assert_eq!(s[2].slot, Some(80.0));
        assert_eq!(s[2].panel_left, Some(-20.0));
    }

    #[test]
    fn the_series_runs_from_the_first_frame_without_a_frames_request() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        let mut st = FrameState::new();
        st.rec.before(at(base, 0));
        st.after(at(base, 3), 10.0, &env);
        assert_eq!(
            st.rec.series().len(),
            1,
            "start-up frames are in the series"
        );
        assert!(matches!(
            st.frames_request(false),
            Reply::Frames { count: 0, .. }
        ));
    }

    #[test]
    fn a_software_series_is_the_redraws_and_not_the_grid_renders() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        let mut st = FrameState::new();
        st.rec.use_redraw_series();
        // A grid render in software mode is not a presented frame: it adds nothing.
        st.rec.before(at(base, 0));
        st.after(at(base, 2), 5.0, &env);
        assert!(st.rec.series().is_empty());
        st.rec
            .redraw_sample(at(base, 10), 100.0, Some((200.0, -20.0)));
        st.rec
            .redraw_sample(at(base, 27), 117.0, Some((150.0, -70.0)));
        let s = st.rec.series();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].gap_ms, None);
        assert!((s[1].gap_ms.unwrap() - 17.0).abs() < 0.5);
        assert_eq!(s[1].render_ms, None, "a redraw request has no render time");
        assert_eq!(s[1].slot, Some(150.0));
    }

    #[test]
    fn a_frames_reset_empties_the_series_and_the_series_stops_at_its_cap() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        let mut st = FrameState::new();
        st.after(at(base, 1), 1.0, &env);
        assert_eq!(st.rec.series().len(), 1);
        assert_eq!(st.frames_request(true), Reply::Ok);
        assert!(st.rec.series().is_empty());
        for i in 0..(SERIES_CAP as u64 + 5) {
            st.after(at(base, i), i as f64, &env);
        }
        assert_eq!(st.rec.series().len(), SERIES_CAP);
    }

    #[test]
    fn the_series_is_json_rows_of_age_gap_render_slot_and_left() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        env.slide.set(Some((150.0, -5.0)));
        let mut st = FrameState::new();
        st.after(at(base, 1), 12.5, &env);
        let v = st.rec.series_json();
        assert_eq!(v[0][0], 12.5);
        assert!(v[0][1].is_null() && v[0][2].is_null());
        assert_eq!(v[0][3], 150.0);
        assert_eq!(v[0][4], -5.0);
    }

    #[test]
    fn the_snapshot_reports_the_slide_its_picture_shows() {
        let mut st = FrameState::new();
        assert!(st.snap_slide_json().is_null(), "no picture taken yet");
        st.set_snap_slide(Some((66.0, 12.0)));
        assert_eq!(st.snap_slide_json(), serde_json::json!([66.0, 12.0]));
        st.set_snap_slide(None);
        assert!(st.snap_slide_json().is_null(), "a picture with no slide");
    }

    #[test]
    fn the_snapshot_reports_the_breath_its_picture_shows() {
        let mut st = FrameState::new();
        assert!(st.snap_breathe_json().is_null(), "no picture taken yet");
        st.set_snap_breathe(Some(shown_breathe(false, 0.25)));
        assert_eq!(st.snap_breathe_json(), serde_json::json!(0.25));
        // A still window draws the breathing marks at full strength, whatever the clock last said.
        st.set_snap_breathe(Some(shown_breathe(true, 0.25)));
        assert_eq!(st.snap_breathe_json(), serde_json::json!(1.0));
        st.set_snap_breathe(None);
        assert!(st.snap_breathe_json().is_null(), "a picture with no window");
    }

    #[test]
    fn latency_completes_on_the_first_frame_that_holds_the_text() {
        let base = Instant::now();
        let env = FakeEnv::showing("$ ", "$");
        let mut st = FrameState::new();
        let (resp, rx) = responder();
        st.arm_latency(LatencyWatch::new(base, "w", 500), None, resp);
        assert!(st.after(at(base, 5), 0.0, &env).is_empty());
        assert!(rx.try_recv().is_err(), "no answer before the text shows");
        env.show("$ w", "$ w");
        st.after(at(base, 12), 0.0, &env);
        match rx.try_recv() {
            Ok(Reply::Latency { ms }) => assert!((ms - 12.0).abs() < 0.5, "{ms}"),
            other => panic!("expected a latency, got {other:?}"),
        }
        env.show("$ ww", "$ ww");
        st.after(at(base, 20), 0.0, &env);
        assert!(rx.try_recv().is_err(), "answered once");
    }

    #[test]
    fn latency_times_out_with_an_error() {
        let base = Instant::now();
        let env = FakeEnv::showing("$ ", "$");
        let mut st = FrameState::new();
        let (resp, rx) = responder();
        let seq = st.arm_latency(LatencyWatch::new(base, "zzz", 50), None, resp);
        st.after(at(base, 60), 0.0, &env);
        assert!(matches!(rx.try_recv(), Ok(Reply::Err { .. })));
        // The timer path answers too, once, and a stale timer does nothing.
        let (resp2, rx2) = responder();
        let seq2 = st.arm_latency(LatencyWatch::new(base, "zzz", 50), None, resp2);
        st.latency_timed_out(seq);
        assert!(rx2.try_recv().is_err(), "a stale timer does not answer");
        st.latency_timed_out(seq2);
        assert!(matches!(rx2.try_recv(), Ok(Reply::Err { .. })));
    }

    #[test]
    fn frames_reset_replies_ok_and_clears() {
        let base = Instant::now();
        let mut st = FrameState::new();
        assert_eq!(st.frames_request(true), Reply::Ok);
        for i in 0..5u64 {
            st.rec.before(at(base, i * 16));
            st.rec.after(at(base, i * 16 + 2));
        }
        match st.frames_request(false) {
            Reply::Frames {
                count,
                max_ms,
                render_p95_ms,
                ..
            } => {
                assert_eq!(count, 5);
                assert!((max_ms - 16.0).abs() < 0.5, "{max_ms}");
                assert!((render_p95_ms - 2.0).abs() < 0.5, "{render_p95_ms}");
            }
            other => panic!("expected frames, got {other:?}"),
        }
        assert_eq!(st.frames_request(true), Reply::Ok);
        assert!(matches!(
            st.frames_request(false),
            Reply::Frames { count: 0, .. }
        ));
    }

    #[test]
    fn key_at_prompt_waits_for_the_suffix_then_injects_once() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "");
        let mut st = FrameState::new();
        st.prompt = Some((PromptWatch::new("> ", Some("a".into())), "KeyA".into()));
        assert!(st.after(at(base, 1), 100.0, &env).is_empty());
        env.show("PS C:\\>", "PS C:\\>");
        assert_eq!(st.after(at(base, 2), 250.0, &env), vec!["KeyA".to_string()]);
        assert_eq!(st.marks.0, Some(250.0));
        // Still the same prompt row: no second injection.
        assert!(st.after(at(base, 3), 260.0, &env).is_empty());
        assert!(st.after(at(base, 4), 270.0, &env).is_empty());
    }

    #[test]
    fn usable_is_prompt_plus_the_echo_frame() {
        let base = Instant::now();
        let env = FakeEnv::showing("", "PS C:\\>");
        let mut st = FrameState::new();
        st.prompt = Some((PromptWatch::new("> ", Some("a".into())), "KeyA".into()));
        st.after(at(base, 1), 250.0, &env);
        env.show("PS C:\\> a", "PS C:\\> a");
        assert!(st.after(at(base, 2), 262.5, &env).is_empty());
        assert_eq!(st.marks, (Some(250.0), Some(262.5)));
        assert!(st.prompt.is_none(), "the watcher is one-shot");
    }
}
