//! Flare's side of the sim host: `SimSource` (macOS), a [`DeviceSource`] that starts `flare-sim-host` guarded (it dies
//! with Flare, rule 1312), talks to it in JSON lines over its stdin and stdout, reads its rings and frame records off
//! the mach channel, sends slot-done records back, restarts it at most [`MAX_RESTARTS`] times in [`RESTART_WINDOW`],
//! and after that (or when it says it is broken) shows still pictures: a `simctl io screenshot` loop at most
//! [`STILL_FPS`] a second, with input and the UI tree answering `unavailable` with the reason.
//!
//! Nothing here blocks the caller (rule 34): every method writes to a channel or starts a worker, and every answer
//! comes back as a [`SourceEvent`].
//!
//! The pure parts work everywhere: the restart policy ([`Restarts`]), the newest-frame mailbox ([`Mailbox`]), the
//! hello check ([`check_hello`]), the still pictures' decoding ([`decode_png`]), and two the sim host runs: input
//! that waits for the device's input connection ([`InputQueue`], up to [`INPUT_WAIT`]) and how text is typed
//! ([`text_steps`]: keys, or the simulator's own pasteboard and Cmd-V, never the Mac's, rule 1314).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::geom::Size;
use crate::keymap::{self, Typing, usage};
use crate::source::Key;
use crate::versions::{self, Support};
use crate::wire::{FrameRecord, Hello, PROTOCOL};

/// The sim host is started again at most this many times in [`RESTART_WINDOW`]; the next end is for good (still
/// mode for the session). The pattern of `flare-mods`' helpers.
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Still pictures: at most this many a second.
pub const STILL_FPS: u32 = 3;
/// The shortest time between two still pictures.
pub const STILL_SPACING: Duration = Duration::from_millis(1000 / STILL_FPS as u64 + 1);
/// The still line's reason when the sim host kept stopping.
pub const KEPT_STOPPING: &str = "the sim host stopped four times in five minutes";

/// When the sim host ended by itself, within the window: whether it may start again.
#[derive(Clone, Debug)]
pub struct Restarts {
    max: usize,
    window: Duration,
    times: Vec<Instant>,
}

impl Default for Restarts {
    fn default() -> Self {
        Self::new(MAX_RESTARTS, RESTART_WINDOW)
    }
}

impl Restarts {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            times: Vec::new(),
        }
    }

    /// It ended at `now`: true when it may be started again (that restart is counted), false when it ended too often.
    pub fn ended(&mut self, now: Instant) -> bool {
        let window = self.window;
        self.times
            .retain(|t| now.saturating_duration_since(*t) < window);
        if self.times.len() >= self.max {
            return false;
        }
        self.times.push(now);
        true
    }
}

/// The newest frame record per ring (`device` number), as the reader thread keeps them for the UI thread. A newer
/// record replaces one the UI has not taken yet: the replaced slot is free again at once (Flare skipped it). The UI
/// is woken once per record it has not taken (the `WakeGate` pattern): one pending wake per device at most.
#[derive(Debug, Default)]
pub struct Mailbox {
    newest: HashMap<u32, FrameRecord>,
    woken: HashSet<u32>,
}

impl Mailbox {
    /// A record came: the record it replaced (to be freed now), and whether to wake the UI.
    pub fn put(&mut self, r: FrameRecord) -> (Option<FrameRecord>, bool) {
        let replaced = self.newest.insert(r.device, r);
        let wake = self.woken.insert(r.device);
        (replaced, wake)
    }

    /// The newest record of `device`, taken (the UI draws it). The next record wakes the UI again.
    pub fn take(&mut self, device: u32) -> Option<FrameRecord> {
        self.woken.remove(&device);
        self.newest.remove(&device)
    }

    /// Whether a record of `device` waits (a wake that could not be sent yet: the ring's target was not known).
    pub fn waiting(&self, device: u32) -> bool {
        self.newest.contains_key(&device)
    }

    /// The ring is gone: its waiting record, to be freed.
    pub fn forget(&mut self, device: u32) -> Option<FrameRecord> {
        self.woken.remove(&device);
        self.newest.remove(&device)
    }
}

/// Whether Flare takes this sim host: the protocol must match (both ship in one bundle, so a mismatch is a broken
/// install), and an Xcode under the floor goes straight to still pictures. Err is the still line's reason.
pub fn check_hello(h: &Hello) -> Result<Support, String> {
    if h.protocol != PROTOCOL {
        return Err(format!(
            "the sim host speaks protocol {}, Flare {PROTOCOL} (reinstall Flare)",
            h.protocol
        ));
    }
    match versions::support(&h.xcode_version, &h.coresim_version) {
        Support::TooOld => Err(versions::TOO_OLD.trim_end_matches('.').to_string()),
        s => Ok(s),
    }
}

/// A still picture (`simctl io screenshot`, a PNG) as RGBA, with its size. None for anything that is not a PNG.
pub fn decode_png(png_bytes: &[u8]) -> Option<(Size, Vec<u8>)> {
    let mut d = png::Decoder::new(std::io::Cursor::new(png_bytes));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };
    (rgba.len() == w * h * 4).then(|| (Size::new(w as f64, h as f64), rgba))
}

/// How long input waits for the device's input connection before it is dropped with a log line (spike S1: the
/// simulator's input service answered up to 11.6 s after a cold boot; the agent's own wait is the same 20 s).
pub const INPUT_WAIT: Duration = Duration::from_secs(20);

/// Input (touches, keys, buttons, text) waiting, in order, for the device's input connection. Each item waits at
/// most [`INPUT_WAIT`] from when it came; then it is dropped, so a finger never lands long after the person or the
/// agent moved on.
#[derive(Debug)]
pub struct InputQueue<T> {
    waiting: VecDeque<(Instant, T)>,
    wait: Duration,
}

impl<T> Default for InputQueue<T> {
    fn default() -> Self {
        InputQueue::new(INPUT_WAIT)
    }
}

impl<T> InputQueue<T> {
    pub fn new(wait: Duration) -> Self {
        InputQueue {
            waiting: VecDeque::new(),
            wait,
        }
    }

    pub fn push(&mut self, item: T, now: Instant) {
        self.waiting.push_back((now, item));
    }

    /// The items that waited longer than the limit, oldest first: they are gone.
    pub fn expire(&mut self, now: Instant) -> Vec<T> {
        let mut out = Vec::new();
        while let Some((t, _)) = self.waiting.front() {
            if now.saturating_duration_since(*t) <= self.wait {
                break;
            }
            if let Some((_, item)) = self.waiting.pop_front() {
                out.push(item);
            }
        }
        out
    }

    /// The connection is live: what still may go, in order, and what waited too long.
    pub fn release(&mut self, now: Instant) -> (Vec<T>, Vec<T>) {
        let dropped = self.expire(now);
        (self.waiting.drain(..).map(|(_, i)| i).collect(), dropped)
    }

    /// When the oldest item runs out of time.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.waiting.front().map(|(t, _)| *t + self.wait)
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

/// One step of typing a text in the simulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextStep {
    /// HID keyboard events, in order.
    Keys(Vec<Key>),
    /// Write the simulator's own pasteboard (`simctl pbcopy <udid>`); the Mac's clipboard is never touched.
    Pasteboard(String),
}

/// Cmd-V: paste the simulator's pasteboard.
pub fn paste_keys() -> Vec<Key> {
    let k = |usage, down| Key { usage, down };
    vec![
        k(usage::LEFT_GUI, true),
        k(usage::V, true),
        k(usage::V, false),
        k(usage::LEFT_GUI, false),
    ]
}

/// How `text` is typed (rule 1314): every character as HID keys with the US keymap, or, when one has no key, the
/// whole text through the simulator's pasteboard and Cmd-V (so its order is kept). Empty text types nothing.
pub fn text_steps(text: &str) -> Vec<TextStep> {
    if text.is_empty() {
        return Vec::new();
    }
    match keymap::typing(text) {
        Typing::Keys(keys) => vec![TextStep::Keys(
            keys.into_iter().flat_map(keymap::events).collect(),
        )],
        Typing::Paste(t) => vec![TextStep::Pasteboard(t), TextStep::Keys(paste_keys())],
    }
}

#[cfg(target_os = "macos")]
pub use mac::{OnEvent, Shoot, SimConfig, SimFrames, SimSource};

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use objc2_core_foundation::CFRetained;
    use objc2_io_surface::IOSurfaceRef;
    use serde_json::Value;

    use super::{KEPT_STOPPING, Mailbox, Restarts, STILL_SPACING, check_hello, decode_png};
    use crate::job::Fail;
    use crate::macos::rendezvous::{Incoming, Peer, RecvError, Server, service_name};
    use crate::macos::simctl::Simctl;
    use crate::source::{
        Answer, Button, DeviceInfo, DeviceSource, Key, ReqId, SourceError, SourceEvent, Touch,
    };
    use crate::target::Target;
    use crate::wire::{
        Control, FrameRecord, Hello, HostLine, Record, Request, SlotDone, from_line, to_line,
    };

    /// A stand-in for `simctl io screenshot` (tests): a PNG of the target's screen.
    pub type Shoot = Arc<dyn Fn(&Target) -> Result<Vec<u8>, String> + Send + Sync>;

    /// Called on the source's own threads after each event is queued (Flare posts one coalesced wake to its UI thread,
    /// which then drains [`DeviceSource::events`]). Never blocks.
    pub type OnEvent = Arc<dyn Fn() + Send + Sync>;

    /// How Flare starts its sim host.
    #[derive(Clone)]
    pub struct SimConfig {
        /// `Contents/MacOS/flare-sim-host` (next to the exe in a dev build).
        pub exe: PathBuf,
        /// The chosen Xcode's `Contents/Developer` (`DEVELOPER_DIR` for the sim host and every `simctl` call).
        pub developer_dir: PathBuf,
        /// `<profile>/logs/sim-host.log`.
        pub log: PathBuf,
        /// Arguments before the sim host's own (tests: the test binary runs one test as the fake sim host).
        pub pre_args: Vec<String>,
        /// More environment for the sim host (tests).
        pub env: Vec<(String, String)>,
        /// The pause before a restart.
        pub restart_delay: Duration,
        pub max_restarts: usize,
        /// Tests: the still pictures' source instead of `simctl`.
        pub shoot: Option<Shoot>,
        /// Told after each event (Flare's wake, PR 4b).
        pub on_event: Option<OnEvent>,
    }

    impl SimConfig {
        pub fn new(exe: PathBuf, developer_dir: PathBuf, log: PathBuf) -> SimConfig {
            SimConfig {
                exe,
                developer_dir,
                log,
                pre_args: Vec::new(),
                env: Vec::new(),
                restart_delay: Duration::from_millis(500),
                max_restarts: super::MAX_RESTARTS,
                shoot: None,
                on_event: None,
            }
        }
    }

    /// What an answer is read as.
    #[derive(Clone, Copy, Debug)]
    enum Kind {
        List,
        Done,
        Png,
        Tree,
    }

    /// One attached target: paused or not.
    #[derive(Clone, Debug)]
    struct Att {
        target: Target,
        paused: bool,
    }

    #[derive(Default)]
    struct State {
        hello: Option<Hello>,
        /// Still mode for the session, and why.
        broken: Option<String>,
        pending: HashMap<ReqId, Kind>,
        /// Lines waiting for a sim host (between restarts).
        queued: Vec<String>,
        attached: Vec<Att>,
        starts: u32,
        stills: HashMap<Target, Arc<AtomicBool>>,
        last_still: HashMap<Target, Vec<u8>>,
    }

    struct Shared {
        cfg: SimConfig,
        tx: Mutex<Sender<SourceEvent>>,
        state: Mutex<State>,
        writer: Mutex<Option<Sender<String>>>,
        guard: Mutex<Option<flare_proc::guard::Guard>>,
        server: Option<Server>,
        expect_pid: AtomicI32,
        peer: Mutex<Option<Peer>>,
        mailbox: Mutex<Mailbox>,
        /// Ring number (`device`) to its target, and each target's current ring.
        devices: Mutex<HashMap<u32, Target>>,
        current: Mutex<HashMap<Target, u32>>,
        rings: Mutex<HashMap<u32, Vec<CFRetained<IOSurfaceRef>>>>,
        /// Slot-done records the sim host's port did not take at once.
        retry: Mutex<Vec<SlotDone>>,
        stop: AtomicBool,
        simctl: Simctl,
        internal_id: AtomicU64,
    }

    // SAFETY: the IOSurfaces are refcounted kernel objects usable from any thread; everything else is behind locks or
    // atomic.
    unsafe impl Send for Shared {}
    // SAFETY: as above.
    unsafe impl Sync for Shared {}

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

        /// Writes a control line to the running sim host, or keeps it for the next one.
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

        fn internal(&self) -> ReqId {
            u64::MAX / 2 + self.internal_id.fetch_add(1, Ordering::Relaxed)
        }

        fn free(&self, f: &FrameRecord) {
            let d = SlotDone {
                device: f.device,
                slot: f.slot,
            };
            self.slot_done(d);
        }

        fn slot_done(&self, d: SlotDone) {
            let sent = lock(&self.peer)
                .as_ref()
                .is_some_and(|p| p.send(&Record::SlotDone(d)));
            if !sent {
                lock(&self.retry).push(d);
            }
        }

        /// Still mode for the session: say why once, and start the pictures of every attached target.
        fn go_still(self: &Arc<Self>, why: String) {
            let targets = {
                let mut st = lock(&self.state);
                if st.broken.is_some() {
                    return;
                }
                st.broken = Some(why.clone());
                for (id, _) in st.pending.drain() {
                    self.fail(
                        id,
                        Fail::Unavailable,
                        format!("Live view unavailable: {why}."),
                    );
                }
                st.queued.clear();
                st.attached
                    .iter()
                    .filter(|a| !a.paused)
                    .map(|a| a.target.clone())
                    .collect::<Vec<_>>()
            };
            log::warn!("device: still pictures for the session: {why}");
            self.event(SourceEvent::Broken(why));
            if let Some(mut g) = lock(&self.guard).take() {
                g.release();
            }
            *lock(&self.writer) = None;
            for t in targets {
                self.start_still(&t);
            }
        }

        /// A still-picture loop for `target`, at most `STILL_FPS` a second, until it is detached or paused.
        fn start_still(self: &Arc<Self>, target: &Target) {
            let flag = Arc::new(AtomicBool::new(false));
            if let Some(old) = lock(&self.state)
                .stills
                .insert(target.clone(), Arc::clone(&flag))
            {
                old.store(true, Ordering::Relaxed);
            }
            let me = Arc::clone(self);
            let target = target.clone();
            std::thread::spawn(move || {
                let mut seq = 0u64;
                while !flag.load(Ordering::Relaxed) && !me.stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    let shot = match &me.cfg.shoot {
                        Some(s) => s(&target),
                        None => me.simctl.screenshot(&target).map_err(|e| e.message),
                    };
                    match shot.as_deref().map(|p| (p, decode_png(p))) {
                        Ok((png, Some((size_px, rgba)))) => {
                            seq += 1;
                            lock(&me.state)
                                .last_still
                                .insert(target.clone(), png.to_vec());
                            me.event(SourceEvent::Picture {
                                target: target.clone(),
                                seq,
                                size_px,
                                rgba,
                            });
                        }
                        Ok((_, None)) => log::warn!("device: {target}: a still that is not a PNG"),
                        Err(e) => log::warn!("device: {target}: no still: {e}"),
                    }
                    let left = STILL_SPACING.saturating_sub(started.elapsed());
                    std::thread::sleep(left); // sync-ok: the still worker's own pacing
                }
            });
        }

        fn stop_still(&self, target: &Target) {
            if let Some(f) = lock(&self.state).stills.remove(target) {
                f.store(true, Ordering::Relaxed);
            }
        }

        /// One line from the sim host.
        fn on_line(self: &Arc<Self>, line: &str) {
            let Some(l) = from_line::<HostLine>(line) else {
                if !line.trim().is_empty() {
                    log::debug!(
                        "device: sim host said {:?}",
                        line.chars().take(200).collect::<String>()
                    );
                }
                return;
            };
            match l {
                HostLine::Hello(h) => match check_hello(&h) {
                    Ok(support) => {
                        log::info!(
                            "device: sim host {} (Xcode {}, CoreSimulator {}, SimulatorKit {}, {:?}, {support:?})",
                            h.version,
                            h.xcode_version,
                            h.coresim_version,
                            h.simkit_version,
                            h.input_path
                        );
                        lock(&self.state).hello = Some(h);
                    }
                    Err(why) => {
                        lock(&self.state).hello = Some(h);
                        self.go_still(why);
                    }
                },
                HostLine::Ok { id, result } => {
                    let kind = lock(&self.state).pending.remove(&id);
                    let answer = match kind {
                        Some(Kind::List) => {
                            let list: Vec<DeviceInfo> = serde_json::from_value(
                                result.get("devices").cloned().unwrap_or(Value::Null),
                            )
                            .unwrap_or_default();
                            Answer::Devices(list)
                        }
                        Some(Kind::Done) => Answer::Done,
                        Some(Kind::Png) => match crate::wire::png_of(&result) {
                            Some(p) => Answer::Png(p),
                            None => {
                                self.fail(id, Fail::Unavailable, "the sim host sent no PNG");
                                return;
                            }
                        },
                        Some(Kind::Tree) => match crate::wire::tree_of(&result) {
                            Some(t) => Answer::Tree(t),
                            None => {
                                self.fail(id, Fail::Unavailable, "the sim host sent no UI tree");
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
                    lock(&self.devices).insert(device, target.clone());
                    let old = lock(&self.current).insert(target.clone(), device);
                    if let Some(old) = old.filter(|o| *o != device) {
                        lock(&self.rings).remove(&old);
                        if let Some(f) = lock(&self.mailbox).forget(old) {
                            self.free(&f);
                        }
                    }
                    self.event(SourceEvent::Frames {
                        target: target.clone(),
                        ring: device,
                        screen,
                    });
                    // A record that came before this line wakes the UI now.
                    let waiting = lock(&self.mailbox).newest_of(device);
                    if let Some(f) = waiting {
                        self.event(frame_event(&target, &f));
                    }
                }
                HostLine::Orientation {
                    target,
                    orientation,
                } => {
                    self.event(SourceEvent::Orientation {
                        target,
                        orientation,
                    });
                }
                HostLine::State { target, state } => {
                    self.event(SourceEvent::State { target, state })
                }
                HostLine::Input { target, state } => {
                    self.event(SourceEvent::Input { target, state })
                }
                HostLine::Broken { why } => self.go_still(why),
            }
        }

        /// The mach channel: rings, frame records, changed counts.
        fn on_mach(&self, m: Incoming) {
            match m {
                Incoming::Hello { reply, protocol } => {
                    if protocol == crate::wire::PROTOCOL {
                        *lock(&self.peer) = Some(reply);
                    }
                }
                Incoming::Ring {
                    device, surfaces, ..
                } => {
                    lock(&self.rings).insert(device, surfaces);
                }
                Incoming::Record(Record::Frame(f)) => {
                    let (replaced, wake) = lock(&self.mailbox).put(f);
                    if let Some(old) = replaced {
                        self.free(&old);
                    }
                    let target = lock(&self.devices).get(&f.device).cloned();
                    if wake && let Some(t) = target {
                        self.event(frame_event(&t, &f));
                    }
                }
                Incoming::Record(Record::Changed(c)) => {
                    if let Some(t) = lock(&self.devices).get(&c.device).cloned() {
                        self.event(SourceEvent::Changed {
                            target: t,
                            count: c.count,
                        });
                    }
                }
                Incoming::Record(Record::SlotDone(_)) => {}
            }
        }
    }

    impl Mailbox {
        /// The newest record of `device`, left in place.
        fn newest_of(&self, device: u32) -> Option<FrameRecord> {
            self.newest.get(&device).copied()
        }
    }

    fn frame_event(target: &Target, f: &FrameRecord) -> SourceEvent {
        SourceEvent::Frame {
            target: target.clone(),
            slot: u8::try_from(f.slot).unwrap_or(u8::MAX),
            seq: f.seq,
            t_us: f.t_us,
        }
    }

    /// Starts the sim host once: the child, its guard, its writer.
    fn spawn(sh: &Arc<Shared>) -> std::io::Result<Child> {
        let cfg = &sh.cfg;
        let mut cmd = Command::new(&cfg.exe); // sync-ok: builds the command; spawned on the supervisor thread
        cmd.args(&cfg.pre_args);
        if let Some(s) = &sh.server {
            cmd.args(["--rendezvous", s.name()]);
        }
        // A sim host started again after one ended ends any finger the old one left down.
        if lock(&sh.state).starts > 0 {
            cmd.arg("--restarted");
        }
        cmd.arg("--log")
            .arg(&cfg.log)
            .env("DEVELOPER_DIR", &cfg.developer_dir)
            .envs(cfg.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let pending = flare_proc::guard::guard_command(&mut cmd)?;
        let mut child = cmd.spawn()?; // sync-ok: the supervisor thread
        *lock(&sh.guard) = Some(pending.started());
        sh.expect_pid
            .store(i32::try_from(child.id()).unwrap_or(0), Ordering::Relaxed);
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
                    log::info!("sim host: {l}");
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

    /// The supervisor: runs the sim host, reads its lines, restarts it, gives up to still mode.
    fn supervise(sh: Arc<Shared>) {
        let mut restarts = Restarts::new(sh.cfg.max_restarts, super::RESTART_WINDOW);
        let mut first = true;
        loop {
            if sh.stop.load(Ordering::Relaxed) || sh.broken().is_some() {
                return;
            }
            let mut child = match spawn(&sh) {
                Ok(c) => c,
                Err(e) => {
                    sh.go_still(format!("the sim host could not start: {e}"));
                    return;
                }
            };
            if !first {
                // Every open panel again: attach, and pause what was paused.
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
            sh.expect_pid.store(0, Ordering::Relaxed);
            *lock(&sh.peer) = None;
            if let Some(mut g) = lock(&sh.guard).take() {
                g.release();
            }
            // What it held is gone with it.
            lock(&sh.devices).clear();
            lock(&sh.current).clear();
            lock(&sh.rings).clear();
            *lock(&sh.mailbox) = Mailbox::default();
            lock(&sh.retry).clear();
            let pending: Vec<ReqId> = lock(&sh.state).pending.drain().map(|(id, _)| id).collect();
            if sh.stop.load(Ordering::Relaxed) || sh.broken().is_some() {
                for id in pending {
                    sh.fail(id, Fail::Unavailable, "the sim host ended");
                }
                return;
            }
            for id in pending {
                sh.fail(id, Fail::Retry, "the sim host restarted");
            }
            log::warn!("device: the sim host ended ({status:?})");
            if !restarts.ended(Instant::now()) {
                sh.go_still(KEPT_STOPPING.to_string());
                return;
            }
            std::thread::sleep(sh.cfg.restart_delay); // sync-ok: the supervisor's pause before a restart
        }
    }

    /// The mach reader: one thread for the whole source.
    fn read_mach(sh: Arc<Shared>) {
        let Some(server) = sh.server.as_ref() else {
            return;
        };
        while !sh.stop.load(Ordering::Relaxed) {
            let pid = sh.expect_pid.load(Ordering::Relaxed);
            if pid <= 0 {
                std::thread::sleep(Duration::from_millis(50)); // sync-ok: the reader waits for a sim host
                continue;
            }
            match server.recv(pid, Duration::from_millis(250)) {
                Ok(m) => sh.on_mach(m),
                Err(RecvError::TimedOut) => {}
                Err(RecvError::Refused { pid, why }) => {
                    log::warn!("device: refused {why} from pid {pid}")
                }
                Err(RecvError::Mach(kr)) => {
                    log::warn!("device: the mach channel failed: kern_return {kr:#x}");
                    std::thread::sleep(Duration::from_millis(250)); // sync-ok: the reader's back-off
                }
            }
            let retry = std::mem::take(&mut *lock(&sh.retry));
            for d in retry {
                sh.slot_done(d);
            }
        }
    }

    /// The frame half of a [`SimSource`], for the render notifier (PR 4b): the newest frame, the ring's surfaces and
    /// the slot-done record. Cheap to clone and `Send`, so a GPU work-done callback can free a slot from any thread.
    #[derive(Clone)]
    pub struct SimFrames {
        sh: Arc<Shared>,
    }

    impl SimFrames {
        /// See [`SimSource::take_frame`].
        pub fn take_frame(&self, target: &Target) -> Option<(u32, FrameRecord)> {
            let device = *lock(&self.sh.current).get(target)?;
            lock(&self.sh.mailbox).take(device).map(|f| (device, f))
        }

        /// See [`SimSource::ring`].
        pub fn ring(&self, device: u32) -> Option<Vec<CFRetained<IOSurfaceRef>>> {
            lock(&self.sh.rings).get(&device).cloned()
        }

        /// See [`SimSource::slot_done`].
        pub fn slot_done(&self, device: u32, slot: u32) {
            self.sh.slot_done(SlotDone { device, slot });
        }
    }

    /// The device source behind the real simulator.
    pub struct SimSource {
        sh: Arc<Shared>,
        rx: Receiver<SourceEvent>,
        next_id: ReqId,
    }

    static CHANNELS: AtomicU32 = AtomicU32::new(1);

    impl SimSource {
        /// Starts the sim host (on its own threads; returns at once). A mach channel that cannot be set up is still
        /// mode from the start (decision 3b: no in-process frame path).
        pub fn start(cfg: SimConfig) -> SimSource {
            let (tx, rx) = channel();
            let server = Server::check_in(&service_name(CHANNELS.fetch_add(1, Ordering::Relaxed)));
            let no_channel = server
                .as_ref()
                .err()
                .map(|e| format!("the ring handover failed: {e}"));
            let sh = Arc::new(Shared {
                simctl: Simctl::new(&cfg.developer_dir),
                cfg,
                tx: Mutex::new(tx),
                state: Mutex::new(State::default()),
                writer: Mutex::new(None),
                guard: Mutex::new(None),
                server: server.ok(),
                expect_pid: AtomicI32::new(0),
                peer: Mutex::new(None),
                mailbox: Mutex::new(Mailbox::default()),
                devices: Mutex::new(HashMap::new()),
                current: Mutex::new(HashMap::new()),
                rings: Mutex::new(HashMap::new()),
                retry: Mutex::new(Vec::new()),
                stop: AtomicBool::new(false),
                internal_id: AtomicU64::new(0),
            });
            if let Some(why) = no_channel {
                sh.go_still(why);
            } else {
                let s = Arc::clone(&sh);
                std::thread::spawn(move || supervise(s));
                let s = Arc::clone(&sh);
                std::thread::spawn(move || read_mach(s));
            }
            SimSource { sh, rx, next_id: 1 }
        }

        fn id(&mut self) -> ReqId {
            let id = self.next_id;
            self.next_id += 1;
            id
        }

        /// The sim host's facts (None until its hello).
        pub fn hello(&self) -> Option<Hello> {
            lock(&self.sh.state).hello.clone()
        }

        /// Still mode, and why (the still line).
        pub fn broken(&self) -> Option<String> {
            self.sh.broken()
        }

        /// How many times the sim host was started.
        pub fn starts(&self) -> u32 {
            lock(&self.sh.state).starts
        }

        /// The newest frame of `target`'s current ring, taken: its ring number and record. The slot is Flare's until
        /// [`SimSource::slot_done`].
        pub fn take_frame(&self, target: &Target) -> Option<(u32, FrameRecord)> {
            self.frames().take_frame(target)
        }

        /// The surfaces of ring `device` (slot order), wrapped as textures by Flare's `DeviceFrames`.
        pub fn ring(&self, device: u32) -> Option<Vec<CFRetained<IOSurfaceRef>>> {
            self.frames().ring(device)
        }

        /// The GPU work that sampled `slot` of ring `device` is done (or Flare skipped it): the slot is free again.
        /// Never blocks: a full port is tried again by the reader.
        pub fn slot_done(&self, device: u32, slot: u32) {
            self.frames().slot_done(device, slot);
        }

        /// The frame half, for the render notifier.
        pub fn frames(&self) -> SimFrames {
            SimFrames {
                sh: Arc::clone(&self.sh),
            }
        }

        fn still_or<F: FnOnce(&Simctl) -> Result<Answer, String> + Send + 'static>(
            &mut self,
            req: Request,
            kind: Kind,
            work: F,
        ) -> ReqId {
            let id = self.id();
            if self.sh.broken().is_some() {
                let sh = Arc::clone(&self.sh);
                std::thread::spawn(move || {
                    let r = work(&sh.simctl).map_err(|m| SourceError {
                        fail: Fail::Unavailable,
                        message: m,
                    });
                    sh.answer(id, r);
                });
            } else {
                self.sh.send(req, id, Some(kind));
            }
            id
        }

        fn unavailable(&mut self, id: ReqId) -> ReqId {
            let why = self.sh.broken().unwrap_or_default();
            self.sh.fail(
                id,
                Fail::Unavailable,
                format!("Live view unavailable: {why}."),
            );
            id
        }
    }

    impl Drop for SimSource {
        fn drop(&mut self) {
            self.sh.stop.store(true, Ordering::Relaxed);
            let id = self.sh.internal();
            self.sh.send(Request::Quit, id, None);
            *lock(&self.sh.writer) = None;
            if let Some(mut g) = lock(&self.sh.guard).take() {
                g.release();
            }
            for (_, f) in lock(&self.sh.state).stills.drain() {
                f.store(true, Ordering::Relaxed);
            }
        }
    }

    impl DeviceSource for SimSource {
        fn list(&mut self) -> ReqId {
            self.still_or(Request::List, Kind::List, |s| {
                s.list().map(Answer::Devices).map_err(|e| e.message)
            })
        }

        fn boot(&mut self, target: &Target) -> ReqId {
            let t = target.clone();
            self.still_or(
                Request::Boot {
                    target: target.clone(),
                },
                Kind::Done,
                move |s| s.boot(&t).map(|()| Answer::Done).map_err(|e| e.message),
            )
        }

        fn shutdown(&mut self, target: &Target) -> ReqId {
            let t = target.clone();
            self.still_or(
                Request::Shutdown {
                    target: target.clone(),
                },
                Kind::Done,
                move |s| s.shutdown(&t).map(|()| Answer::Done).map_err(|e| e.message),
            )
        }

        fn attach(&mut self, target: &Target) -> ReqId {
            let id = self.id();
            {
                let mut st = lock(&self.sh.state);
                if !st.attached.iter().any(|a| &a.target == target) {
                    st.attached.push(Att {
                        target: target.clone(),
                        paused: false,
                    });
                }
            }
            if self.sh.broken().is_some() {
                self.sh.start_still(target);
                self.sh.answer(id, Ok(Answer::Done));
            } else {
                self.sh.send(
                    Request::Attach {
                        target: target.clone(),
                    },
                    id,
                    Some(Kind::Done),
                );
            }
            id
        }

        fn detach(&mut self, target: &Target) {
            lock(&self.sh.state)
                .attached
                .retain(|a| &a.target != target);
            self.sh.stop_still(target);
            if self.sh.broken().is_none() {
                let id = self.sh.internal();
                self.sh.send(
                    Request::Detach {
                        target: target.clone(),
                    },
                    id,
                    None,
                );
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
            if self.sh.broken().is_some() {
                if paused {
                    self.sh.stop_still(target);
                } else {
                    self.sh.start_still(target);
                }
                return;
            }
            let id = self.sh.internal();
            let req = if paused {
                Request::Pause {
                    target: target.clone(),
                }
            } else {
                Request::Resume {
                    target: target.clone(),
                }
            };
            self.sh.send(req, id, None);
        }

        fn touch(&mut self, target: &Target, touch: Touch) {
            if self.sh.broken().is_none() {
                let id = self.sh.internal();
                self.sh.send(
                    Request::Touch {
                        target: target.clone(),
                        touch,
                    },
                    id,
                    None,
                );
            }
        }

        fn key(&mut self, target: &Target, key: Key) {
            if self.sh.broken().is_none() {
                let id = self.sh.internal();
                self.sh.send(
                    Request::Key {
                        target: target.clone(),
                        key,
                    },
                    id,
                    None,
                );
            }
        }

        fn button(&mut self, target: &Target, button: Button) {
            if self.sh.broken().is_none() {
                let id = self.sh.internal();
                self.sh.send(
                    Request::Button {
                        target: target.clone(),
                        button,
                    },
                    id,
                    None,
                );
            }
        }

        fn text(&mut self, target: &Target, text: &str) -> ReqId {
            let id = self.id();
            if self.sh.broken().is_some() {
                return self.unavailable(id);
            }
            self.sh.send(
                Request::Text {
                    target: target.clone(),
                    text: text.to_string(),
                },
                id,
                Some(Kind::Done),
            );
            id
        }

        fn snapshot(&mut self, target: &Target) -> ReqId {
            let id = self.id();
            if self.sh.broken().is_some() {
                // The latest still (the agent's `device_look` in still mode).
                let png = lock(&self.sh.state).last_still.get(target).cloned();
                match png {
                    Some(p) => self.sh.answer(id, Ok(Answer::Png(p))),
                    None => {
                        self.unavailable(id);
                    }
                }
                return id;
            }
            self.sh.send(
                Request::Snapshot {
                    target: target.clone(),
                },
                id,
                Some(Kind::Png),
            );
            id
        }

        fn ui_tree(&mut self, target: &Target) -> ReqId {
            let id = self.id();
            if self.sh.broken().is_some() {
                return self.unavailable(id);
            }
            self.sh.send(
                Request::UiTree {
                    target: target.clone(),
                    at: None,
                },
                id,
                Some(Kind::Tree),
            );
            id
        }

        fn launch(&mut self, target: &Target, bundle_id: &str) -> ReqId {
            let (t, b) = (target.clone(), bundle_id.to_string());
            self.still_or(
                Request::Launch {
                    target: target.clone(),
                    bundle_id: bundle_id.to_string(),
                    args: Vec::new(),
                },
                Kind::Done,
                move |s| {
                    s.launch(&t, &b, &[])
                        .map(|()| Answer::Done)
                        .map_err(|e| e.message)
                },
            )
        }

        fn install(&mut self, target: &Target, app: &Path) -> ReqId {
            let (t, a) = (target.clone(), app.to_path_buf());
            self.still_or(
                Request::Install {
                    target: target.clone(),
                    path: app.to_string_lossy().to_string(),
                },
                Kind::Done,
                move |s| {
                    s.install(&t, &a)
                        .map(|()| Answer::Done)
                        .map_err(|e| e.message)
                },
            )
        }

        fn events(&self) -> &Receiver<SourceEvent> {
            &self.rx
        }

        fn hello(&self) -> Option<Hello> {
            SimSource::hello(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::FrameRecord;

    fn rec(device: u32, slot: u32, seq: u64) -> FrameRecord {
        FrameRecord {
            device,
            slot,
            seq,
            t_us: seq * 16_667,
        }
    }

    #[test]
    fn frame_records_keep_only_the_newest_per_device() {
        let mut m = Mailbox::default();
        assert_eq!(
            m.put(rec(1, 0, 1)),
            (None, true),
            "the first record wakes the UI"
        );
        assert_eq!(
            m.put(rec(1, 1, 2)),
            (Some(rec(1, 0, 1)), false),
            "a newer one replaces it (its slot is freed at once) and does not wake again"
        );
        assert_eq!(m.put(rec(2, 0, 9)), (None, true), "each device has its own");
        assert_eq!(m.put(rec(1, 2, 3)), (Some(rec(1, 1, 2)), false));
        assert_eq!(m.take(1), Some(rec(1, 2, 3)), "the UI gets only the newest");
        assert_eq!(m.take(1), None);
        assert_eq!(
            m.put(rec(1, 0, 4)),
            (None, true),
            "taken: the next record wakes again"
        );
        assert!(m.waiting(2));
        assert_eq!(
            m.forget(2),
            Some(rec(2, 0, 9)),
            "a ring gone gives its slot back"
        );
        assert!(!m.waiting(2));
        // The restart policy: three restarts in five minutes, the fourth end is for good; an old end ages out.
        let mut r = Restarts::default();
        let t0 = Instant::now();
        assert!(r.ended(t0));
        assert!(r.ended(t0 + Duration::from_secs(1)));
        assert!(r.ended(t0 + Duration::from_secs(2)));
        assert!(!r.ended(t0 + Duration::from_secs(3)));
        let mut r = Restarts::default();
        for i in 0..3 {
            assert!(r.ended(t0 + Duration::from_secs(i)));
        }
        assert!(
            r.ended(t0 + RESTART_WINDOW + Duration::from_secs(1)),
            "the first end aged out"
        );
        assert!(STILL_SPACING >= Duration::from_millis(1000 / u64::from(STILL_FPS)));
    }

    #[test]
    fn touches_wait_up_to_twenty_seconds_for_the_connection_then_drop() {
        assert_eq!(INPUT_WAIT, Duration::from_secs(20));
        let s = Duration::from_secs;
        let t0 = Instant::now();
        let mut q = InputQueue::default();
        q.push("start", t0);
        q.push("end", t0 + s(1));
        assert!(
            q.expire(t0 + s(19)).is_empty(),
            "nothing is dropped before 20 s"
        );
        assert_eq!(q.len(), 2);
        // The connection is live at 19.5 s: both go, in order.
        let (go, dropped) = q.release(t0 + Duration::from_millis(19_500));
        assert_eq!(go, ["start", "end"]);
        assert!(dropped.is_empty());
        assert!(q.is_empty());
        // A touch that waited past 20 s is dropped; a later one waits on.
        q.push("a", t0);
        q.push("b", t0 + s(5));
        assert_eq!(q.next_deadline(), Some(t0 + s(20)));
        assert_eq!(q.expire(t0 + Duration::from_millis(20_500)), ["a"]);
        assert_eq!(q.next_deadline(), Some(t0 + s(25)));
        // Still no connection at 26 s: the rest is dropped too, nothing goes.
        let (go, dropped) = q.release(t0 + s(26));
        assert!(go.is_empty());
        assert_eq!(dropped, ["b"]);
    }

    #[test]
    fn non_keymap_text_goes_through_the_simulator_pasteboard_not_the_macs() {
        use crate::keymap::usage;
        // Keymap text is typed as keys, shift included; nothing is pasted.
        let k = |usage, down| Key { usage, down };
        assert_eq!(
            text_steps("Hi"),
            vec![TextStep::Keys(vec![
                k(usage::LEFT_SHIFT, true),
                k(0x0B, true),
                k(0x0B, false),
                k(usage::LEFT_SHIFT, false),
                k(0x0C, true),
                k(0x0C, false),
            ])]
        );
        // Text with a character the keymap lacks goes whole through the simulator's pasteboard, then Cmd-V.
        assert_eq!(
            text_steps("caf\u{e9} ok"),
            vec![
                TextStep::Pasteboard("caf\u{e9} ok".into()),
                TextStep::Keys(vec![
                    k(usage::LEFT_GUI, true),
                    k(usage::V, true),
                    k(usage::V, false),
                    k(usage::LEFT_GUI, false),
                ]),
            ]
        );
        assert!(text_steps("").is_empty());
        // Nothing in this crate reaches the Mac's clipboard (the needles are split so this file does not match).
        let needles = [
            concat!("NS", "Pasteboard"),
            concat!("/usr/bin/", "pbcopy"),
            concat!("/usr/bin/", "pbpaste"),
            concat!("Command::new(\"", "pb"),
            concat!("general", "Pasteboard"),
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.clone()];
        let mut seen = 0;
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
            } else if p.extension().is_some_and(|e| e == "rs") {
                seen += 1;
                let text = std::fs::read_to_string(&p).unwrap();
                for n in needles {
                    assert!(!text.contains(n), "{} uses {n}", p.display());
                }
            }
        }
        assert!(seen > 10, "the crate's sources were read");
        #[cfg(target_os = "macos")]
        {
            // The pasteboard written is the simulator's: `xcrun simctl pbcopy <udid>`.
            let t = target();
            let c = crate::macos::simctl::Simctl::new(Path::new(
                "/Applications/Xcode.app/Contents/Developer",
            ))
            .pbcopy_command(&t);
            assert_eq!(c.get_program(), "/usr/bin/xcrun");
            let args: Vec<_> = c
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            assert_eq!(args, ["simctl", "pbcopy", &t.udid().to_ascii_uppercase()]);
            // And Flare hands such text to the sim host, which answers once it is pasted.
            let dir = tempfile::tempdir().unwrap();
            let mut s = source("record", dir.path(), None);
            let id = s.text(&t, "caf\u{e9}");
            let seen = until(
                &s,
                Duration::from_secs(30),
                |e| matches!(e, SourceEvent::Answer { id: i, .. } if *i == id),
            );
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
            let lines = std::fs::read_to_string(dir.path().join("lines")).unwrap();
            let line = lines
                .lines()
                .find(|l| l.contains("\"op\":\"text\""))
                .unwrap();
            let c: crate::wire::Control = crate::wire::from_line(line).unwrap();
            assert_eq!(
                c.req,
                crate::wire::Request::Text {
                    target: t,
                    text: "caf\u{e9}".into()
                }
            );
        }
    }

    fn hello(protocol: u32, xcode: &str) -> Hello {
        Hello {
            protocol,
            version: "0.1.0".into(),
            coresim_version: "1174.21".into(),
            simkit_version: "1005.14".into(),
            xcode_version: xcode.into(),
            xcode_path: "/Applications/Xcode.app/Contents/Developer".into(),
            input_path: crate::versions::InputPath::Dtuhid,
            ax: false,
        }
    }

    /// A 2x1 PNG (one colour per pixel), as `simctl io screenshot` would give.
    pub(crate) fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, 2, 1);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 0, 0, 255]).unwrap();
        }
        out
    }

    #[test]
    fn a_protocol_mismatch_in_hello_is_refused() {
        assert_eq!(check_hello(&hello(PROTOCOL, "27.2")), Ok(Support::Tested));
        assert_eq!(check_hello(&hello(PROTOCOL, "27.9")), Ok(Support::Untested));
        let e = check_hello(&hello(PROTOCOL + 1, "27.2")).unwrap_err();
        assert!(e.contains("protocol 2") && e.contains("reinstall"), "{e}");
        assert_eq!(
            check_hello(&hello(PROTOCOL, "16.4")),
            Err("This Xcode is too old for the live view".to_string())
        );
        // And a still picture decodes to RGBA.
        let (size, rgba) = decode_png(&tiny_png()).unwrap();
        assert_eq!(size, Size::new(2.0, 1.0));
        assert_eq!(rgba, [255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(decode_png(b"\x89PNG\r\n\x1a\nflare-fake-device"), None);
        #[cfg(target_os = "macos")]
        a_mismatched_sim_host_is_not_restarted();
    }

    #[cfg(target_os = "macos")]
    use crate::source::{Answer, DeviceSource, SourceEvent};
    #[cfg(target_os = "macos")]
    use crate::target::Target;
    #[cfg(target_os = "macos")]
    use std::io::{BufRead, Write};
    #[cfg(target_os = "macos")]
    use std::path::Path;
    #[cfg(target_os = "macos")]
    use std::sync::Arc;

    #[cfg(target_os = "macos")]
    /// Not a test on its own: a guard started by these tests runs as this test.
    #[test]
    fn guard_entry() {
        flare_proc::guard::run_if_guard();
    }

    #[cfg(target_os = "macos")]
    /// Not a test on its own: run by the tests below as a fake `flare-sim-host` (`FLARE_FAKE_SIM_HOST` says how
    /// it behaves).
    #[test]
    fn fake_sim_host() {
        let Ok(mode) = std::env::var("FLARE_FAKE_SIM_HOST") else {
            return;
        };
        let dir = std::env::var("FLARE_FAKE_SIM_DIR").unwrap_or_default();
        let args: Vec<String> = std::env::args().collect();
        let rendezvous = args
            .iter()
            .position(|a| a == "--rendezvous")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(&dir).join("starts"))
        {
            let _ = writeln!(f, "{}", std::process::id());
        }
        let mut h = hello(PROTOCOL, "27.2");
        match mode.as_str() {
            "mismatch" => h.protocol = 99,
            "fds" => {
                // Every descriptor open in this process (stdin, stdout and stderr only, or the guard leaked one).
                let open: Vec<String> = (0..1024)
                    // SAFETY: F_GETFD on any number only asks; -1 means not open.
                    .filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } != -1)
                    .map(|fd: i32| fd.to_string())
                    .collect();
                h.version = format!("fds={}", open.join(","));
                h.xcode_path = std::env::var("DEVELOPER_DIR").unwrap_or_default();
                h.simkit_version = rendezvous;
            }
            _ => {}
        }
        let mut out = std::io::stdout();
        let _ = writeln!(
            out,
            "{}",
            crate::wire::to_line(&crate::wire::HostLine::Hello(h))
        );
        let _ = out.flush();
        if mode == "crash" {
            std::process::exit(1);
        }
        // Wait for Flare to go (quit, or the pipe closes). "record" keeps every line and answers it.
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if line.contains("\"op\":\"quit\"") {
                break;
            }
            if mode == "record" {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(Path::new(&dir).join("lines"))
                {
                    let _ = writeln!(f, "{line}");
                }
                if let Some(c) = crate::wire::from_line::<crate::wire::Control>(&line) {
                    let _ = writeln!(
                        out,
                        "{}",
                        crate::wire::to_line(&crate::wire::HostLine::Ok {
                            id: c.id,
                            result: serde_json::Value::Null
                        })
                    );
                    let _ = out.flush();
                }
            }
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    fn source(mode: &str, dir: &Path, shoot: Option<Shoot>) -> SimSource {
        flare_proc::guard::use_test_entry("client::tests::guard_entry").unwrap();
        let mut cfg = SimConfig::new(
            std::env::current_exe().unwrap(),
            dir.join("Xcode.app/Contents/Developer"),
            dir.join("logs/sim-host.log"),
        );
        cfg.pre_args = [
            "client::tests::fake_sim_host",
            "--exact",
            "--nocapture",
            "--",
        ]
        .map(String::from)
        .to_vec();
        cfg.env = vec![
            ("FLARE_FAKE_SIM_HOST".into(), mode.into()),
            (
                "FLARE_FAKE_SIM_DIR".into(),
                dir.to_string_lossy().to_string(),
            ),
        ];
        cfg.restart_delay = Duration::from_millis(50);
        cfg.shoot = shoot;
        SimSource::start(cfg)
    }

    #[cfg(target_os = "macos")]
    fn starts(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("starts")).map_or(0, |t| t.lines().count())
    }

    #[cfg(target_os = "macos")]
    /// Events until `want` matches one, within `limit`; every event seen.
    fn until(
        s: &SimSource,
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

    #[cfg(target_os = "macos")]
    fn target() -> Target {
        Target::sim("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sim_host_is_restarted_three_times_then_broken() {
        let dir = tempfile::tempdir().unwrap();
        let s = source("crash", dir.path(), None);
        let seen = until(&s, Duration::from_secs(60), |e| {
            matches!(e, SourceEvent::Broken(_))
        });
        let Some(SourceEvent::Broken(why)) = seen.last() else {
            unreachable!()
        };
        assert_eq!(why, KEPT_STOPPING);
        assert_eq!(
            starts(dir.path()),
            1 + MAX_RESTARTS,
            "started once, then three restarts"
        );
        assert_eq!(s.starts() as usize, 1 + MAX_RESTARTS);
        assert_eq!(s.broken().as_deref(), Some(KEPT_STOPPING));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            starts(dir.path()),
            1 + MAX_RESTARTS,
            "no start after it is broken"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_broken_sim_host_falls_back_to_still_pictures_at_three_fps_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let shots = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = Arc::clone(&shots);
        let shoot: Shoot = Arc::new(move |_t: &Target| {
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(tiny_png())
        });
        let mut s = source("crash", dir.path(), Some(shoot));
        let t = target();
        let attach = s.attach(&t);
        let seen = until(&s, Duration::from_secs(60), |e| {
            matches!(e, SourceEvent::Broken(_))
        });
        let mut answered_attach = seen
            .iter()
            .any(|e| matches!(e, SourceEvent::Answer { id, .. } if *id == attach));
        // The pictures come, no faster than three a second.
        let started = Instant::now();
        let mut times = Vec::new();
        while started.elapsed() < Duration::from_millis(2100) {
            match s.events().recv_timeout(Duration::from_millis(100)) {
                Ok(SourceEvent::Picture {
                    target,
                    size_px,
                    rgba,
                    ..
                }) => {
                    assert_eq!(target, t);
                    assert_eq!(size_px, Size::new(2.0, 1.0));
                    assert_eq!(rgba.len(), 8);
                    times.push(Instant::now());
                }
                Ok(SourceEvent::Answer { id, .. }) if id == attach => answered_attach = true,
                _ => {}
            }
        }
        assert!(
            answered_attach,
            "the attach was answered (retry, then still)"
        );
        assert!(
            (4..=7).contains(&times.len()),
            "{} pictures in 2.1 s",
            times.len()
        );
        for w in times.windows(2) {
            assert!(
                w[1] - w[0] >= Duration::from_millis(300),
                "{:?}",
                w[1] - w[0]
            );
        }
        // Input and the UI tree answer unavailable with the reason; a look gets the latest still.
        let tree = s.ui_tree(&t);
        let look = s.snapshot(&t);
        let mut got = HashMap::new();
        while got.len() < 2 {
            if let Ok(SourceEvent::Answer { id, result }) =
                s.events().recv_timeout(Duration::from_secs(5))
            {
                got.insert(id, result);
            }
        }
        let e = got[&tree].clone().unwrap_err();
        assert_eq!(e.fail, crate::job::Fail::Unavailable);
        assert!(e.message.contains(KEPT_STOPPING), "{}", e.message);
        assert_eq!(got[&look], Ok(Answer::Png(tiny_png())));
        // Detached: the pictures stop.
        s.detach(&t);
        let n = shots.load(std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(800));
        assert!(shots.load(std::sync::atomic::Ordering::Relaxed) <= n + 1);
    }

    #[cfg(target_os = "macos")]
    fn a_mismatched_sim_host_is_not_restarted() {
        let dir = tempfile::tempdir().unwrap();
        let s = source("mismatch", dir.path(), None);
        let seen = until(&s, Duration::from_secs(30), |e| {
            matches!(e, SourceEvent::Broken(_))
        });
        let Some(SourceEvent::Broken(why)) = seen.last() else {
            unreachable!()
        };
        assert!(why.contains("protocol 99"), "{why}");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(starts(dir.path()), 1, "a broken install is not restarted");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sim_host_gets_developer_dir_and_no_extra_descriptors() {
        let dir = tempfile::tempdir().unwrap();
        let s = source("fds", dir.path(), None);
        let end = Instant::now() + Duration::from_secs(30);
        let hello = loop {
            if let Some(h) = s.hello() {
                break h;
            }
            assert!(Instant::now() < end, "no hello");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(hello.version, "fds=0,1,2", "stdin, stdout and stderr only");
        assert_eq!(
            Path::new(&hello.xcode_path),
            dir.path().join("Xcode.app/Contents/Developer")
        );
        assert!(
            hello.simkit_version.starts_with(&format!(
                "{}{}.",
                crate::macos::rendezvous::SERVICE_PREFIX,
                std::process::id()
            )),
            "the rendezvous name, with Flare's pid: {}",
            hello.simkit_version
        );
        assert_eq!(s.broken(), None);
    }
}
