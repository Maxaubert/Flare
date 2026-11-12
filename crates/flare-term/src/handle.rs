//! TermHandle: one tab's terminal (emulator + session + input encoding + wake).
//!
//! Output arrives on the session's delivery thread (`SessionSink::data`), is parsed under alacritty's
//! FAIR lock in slices of at most 64 KiB (so the UI's `snapshot` gets its turn between slices), and
//! every reply the emulator makes goes back to the pty without counting as typing (rule 9, rule 10).

use crate::emulator::{DecorReq, Emulator, ModeSnapshot, TermEvent};
use crate::keys::KeyEvent;
use crate::replies::bell_gate;
use crate::session::{SessionManager, SessionSink};
use crate::wheel::{WheelAction, WheelContext, wheel_action};
use alacritty_terminal::sync::FairMutex;
use flare_agent::reveal::{self, RevealState};
use flare_render::decor::Decor;
use flare_shells::env::PluginEnv;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

/// The largest slice of output parsed under one hold of the emulator lock.
const SLICE: usize = 64 * 1024;
/// The rows a copy worker reads under one hold of the emulator lock (the lock is let go between them).
pub const CHUNK_ROWS: usize = 250;

/// Where the cursor stood when one parsed slice of output ended (screen row, column, wall-clock ms).
/// A frame may show only the last of several such stops; the caret hold needs each of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorStop {
    pub line: i32,
    pub col: usize,
    pub at_ms: i64,
}

/// The most stops kept between two frames.
const STOPS_KEPT: usize = 64;

/// What a plugin that holds `input` may write into a shell (rule 2): a typed intent, never raw bytes. A paste goes
/// the way the person's paste goes; an Enter is the carriage return that sends what was pasted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginInput<'a> {
    Paste(&'a str),
    Enter,
}

/// What a remote viewer typed on a host tab (remote sessions PR 2, rule 279): the second typed path beside the plugin's,
/// never raw bytes. A key goes through the same encoder a local key does, with this tab's modes; a text holds no
/// control character (one that does is dropped); a paste is sanitised and goes the way a local paste goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteIntent {
    Key(KeyEvent),
    Text(String),
    Paste(String),
}

/// Who typed last in a tab: this machine (the person, dictation, a plugin) or a remote viewer. The size of a tab follows
/// the last typist (rule 282).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typist {
    Local,
    Remote,
}

const TYPIST_NONE: u8 = 0;
const TYPIST_LOCAL: u8 = 1;
const TYPIST_REMOTE: u8 = 2;

/// A tab's screen and the history above it as text, with the number of the tail's next byte (`remote.snapshot`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSnapshot {
    /// The tail's number when the text was read: `remote.output {since: seq}` goes on from here.
    pub seq: u64,
    pub cols: usize,
    pub rows: usize,
    /// Clear, home, then the lines joined by CR LF: what a fresh emulator is fed to show the same text.
    pub data: Vec<u8>,
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    pub root: String,
    pub shell_id: Option<String>,
    pub resume: Option<String>,
    pub plugin: Option<PluginEnv>,
    /// Some for a remote tab (remote sessions PR 3): the session is the host's, shown through a `RemotePty`. The
    /// shell, the resume and the plugin are then the host's business and are not used here.
    pub remote: Option<crate::remote_pty::RemoteSpec>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

struct Shared {
    id: String,
    emu: Arc<FairMutex<Emulator>>,
    mgr: Arc<SessionManager>,
    events: Mutex<Vec<TermEvent>>,
    dirty: AtomicBool,
    waker: Mutex<Option<Waker>>,
    last_typed: Mutex<Option<Instant>>,
    last_bell: Mutex<Option<u64>>,
    sync_armed: AtomicBool,
    output_pending: AtomicBool,
    t0: Instant,
    reveal: Mutex<Option<RevealState>>,
    clock: Mutex<Option<Clock>>,
    /// Where the cursor stood at the end of each parsed slice, for the IME caret hold.
    cursor_log: Mutex<Vec<CursorStop>>,
    /// Who typed last (`TYPIST_*`), read by the tab host's size owner (rule 282).
    typist: AtomicU8,
}

impl Shared {
    /// Milliseconds on the reveal's clock: the injected one, else since the handle was made.
    fn now(&self) -> u64 {
        let c = lk(&self.clock).clone();
        match c {
            Some(f) => f(),
            None => self.t0.elapsed().as_millis() as u64,
        }
    }

    fn resuming(&self) -> bool {
        lk(&self.reveal).as_ref().is_some_and(|r| !r.revealed)
    }

    /// Applies `f` to a live reveal; queues `Revealed` once when it flips.
    fn reveal_step(&self, f: impl FnOnce(&RevealState) -> RevealState) {
        let flipped = {
            let mut g = lk(&self.reveal);
            let Some(cur) = g.as_ref().filter(|r| !r.revealed) else {
                return;
            };
            let next = f(cur);
            let flipped = next.revealed;
            *g = Some(next);
            flipped
        };
        if flipped {
            lk(&self.events).push(TermEvent::Revealed);
            self.wake();
        }
    }

    fn finish_reveal(&self) {
        self.reveal_step(reveal::reveal_now);
    }

    fn wake(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        let w = lk(&self.waker).clone();
        if let Some(w) = w {
            w();
        }
    }

    /// Replies go back to the pty (not typing); a bell passes once a second; the rest is queued.
    fn handle(&self, evs: Vec<TermEvent>) {
        for ev in evs {
            match ev {
                TermEvent::Reply(b) => self.mgr.write_reply(&self.id, &b),
                TermEvent::Bell => {
                    let now = self.t0.elapsed().as_millis() as u64;
                    let mut last = lk(&self.last_bell);
                    if bell_gate(*last, now) {
                        *last = Some(now);
                        drop(last);
                        lk(&self.events).push(TermEvent::Bell);
                    }
                }
                other => lk(&self.events).push(other),
            }
        }
    }

    /// Typing: `Typed` is queued and the next output queues a fresh `Output` after it.
    fn mark_typed(&self) {
        self.mark_typed_by(Typist::Local);
    }

    fn mark_typed_by(&self, by: Typist) {
        self.typist.store(
            match by {
                Typist::Local => TYPIST_LOCAL,
                Typist::Remote => TYPIST_REMOTE,
            },
            Ordering::SeqCst,
        );
        *lk(&self.last_typed) = Some(Instant::now());
        let mut evs = lk(&self.events);
        evs.push(TermEvent::Typed);
        self.output_pending.store(false, Ordering::SeqCst);
    }

    /// One `Output` per drained batch: queued only when none is waiting.
    fn note_output(&self) {
        let mut evs = lk(&self.events);
        if !self.output_pending.swap(true, Ordering::SeqCst) {
            evs.push(TermEvent::Output);
        }
    }

    fn snap_to_bottom(&self) {
        self.emu.lock().scroll_to_bottom();
        self.dirty.store(true, Ordering::SeqCst);
    }
}

/// What the session manager talks to. Weak, so a closed tab is not kept alive by its own session.
struct Sink(Weak<Shared>);

impl SessionSink for Sink {
    fn data(&self, _id: &str, bytes: Vec<u8>) {
        let Some(sh) = self.0.upgrade() else { return };
        // The reveal reads the WHOLE chunk once, so a title is never cut by a slice boundary.
        let clear_at = if sh.resuming() {
            let now = sh.now();
            let mut clear_at: Option<usize> = None;
            sh.reveal_step(|st| {
                let (next, at) = reveal::on_chunk(st, &bytes, now);
                clear_at = at;
                next
            });
            clear_at.map(|at| at.min(bytes.len()))
        } else {
            None
        };
        let was_alt = sh.emu.lock().mode().alt_screen;
        match clear_at {
            Some(at) => {
                feed_slices(&sh, &bytes[..at]);
                // The clear is the GRID's only: it is never given to the pty.
                let evs = sh.emu.lock().feed(reveal::CLEAR);
                sh.handle(
                    evs.into_iter()
                        .filter(|e| !matches!(e, TermEvent::Reply(_)))
                        .collect(),
                );
                feed_slices(&sh, &bytes[at..]);
            }
            None => feed_slices(&sh, &bytes),
        }
        if sh.resuming() {
            let now = sh.now();
            if !was_alt && sh.emu.lock().mode().alt_screen {
                sh.reveal_step(|st| reveal::on_alternate(st, now));
            }
            sh.reveal_step(|st| reveal::on_tick(st, now));
        }
        sh.note_output();
        sh.wake();
        arm_sync(&sh);
    }

    fn exit(&self, _id: &str, code: Option<u32>) {
        let Some(sh) = self.0.upgrade() else { return };
        sh.finish_reveal();
        lk(&sh.events).push(TermEvent::Exited(code));
        sh.wake();
    }
}

fn feed_slices(sh: &Shared, bytes: &[u8]) {
    for slice in bytes.chunks(SLICE) {
        let (evs, stop) = {
            let mut e = sh.emu.lock();
            let evs = e.feed(slice);
            let stop = (!e.mode().alt_screen).then(|| e.cursor());
            (evs, stop)
        };
        if let Some((line, col)) = stop {
            let mut log = lk(&sh.cursor_log);
            if log.len() >= STOPS_KEPT {
                log.remove(0);
            }
            log.push(CursorStop {
                line,
                col,
                at_ms: epoch_ms(),
            });
        }
        sh.handle(evs);
    }
}

/// A synchronized update (DEC 2026) holds bytes back until its deadline; `vte` only stores the
/// deadline, so one short-lived thread lets it go (decision 15).
fn arm_sync(sh: &Arc<Shared>) {
    if sh.emu.lock().sync_deadline().is_none() || sh.sync_armed.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(sh);
    let r = std::thread::Builder::new()
        .name("flare-sync".into())
        .spawn(move || sync_loop(weak));
    if let Err(e) = r {
        log::warn!("could not start the sync timer: {e}");
        sh.sync_armed.store(false, Ordering::SeqCst);
    }
}

fn sync_loop(weak: Weak<Shared>) {
    loop {
        loop {
            let Some(sh) = weak.upgrade() else { return };
            let Some(d) = sh.emu.lock().sync_deadline() else {
                break;
            };
            drop(sh);
            std::thread::sleep(d.saturating_duration_since(Instant::now())); // sync-ok: the sync timer thread, never the UI
            let Some(sh) = weak.upgrade() else { return };
            let evs = sh.emu.lock().release_sync_if_due(Instant::now());
            sh.handle(evs);
            sh.wake();
        }
        let Some(sh) = weak.upgrade() else { return };
        sh.sync_armed.store(false, Ordering::SeqCst);
        // A deadline set in between re-arms this same thread.
        if sh.emu.lock().sync_deadline().is_some() && !sh.sync_armed.swap(true, Ordering::SeqCst) {
            continue;
        }
        return;
    }
}

pub struct TermHandle {
    sh: Arc<Shared>,
    emu: Arc<FairMutex<Emulator>>,
}

impl TermHandle {
    pub fn new(id: &str, mgr: Arc<SessionManager>, cols: usize, rows: usize) -> Arc<Self> {
        let emu = Arc::new(FairMutex::new(Emulator::new(cols, rows)));
        let sh = Arc::new(Shared {
            id: id.to_string(),
            emu: Arc::clone(&emu),
            mgr,
            events: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(true),
            waker: Mutex::new(None),
            last_typed: Mutex::new(None),
            last_bell: Mutex::new(None),
            sync_armed: AtomicBool::new(false),
            output_pending: AtomicBool::new(false),
            t0: Instant::now(),
            reveal: Mutex::new(None),
            clock: Mutex::new(None),
            cursor_log: Mutex::new(Vec::new()),
            typist: AtomicU8::new(TYPIST_NONE),
        });
        Arc::new(Self { sh, emu })
    }

    pub fn id(&self) -> &str {
        &self.sh.id
    }

    /// Called from the parse thread after output: the UI posts a redraw. Replaced if set again.
    pub fn set_waker(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *lk(&self.sh.waker) = Some(f);
    }

    /// The reveal's clock, in ms (a test seam). Default: milliseconds since the handle was made.
    pub fn set_clock(&self, f: Arc<dyn Fn() -> u64 + Send + Sync>) {
        *lk(&self.sh.clock) = Some(f);
    }

    /// True from `start` with a resume until the agent has painted (or a key, an exit, a failed
    /// spawn, the ceiling).
    pub fn resuming(&self) -> bool {
        self.sh.resuming()
    }

    /// The glue calls it on its timer while `resuming()`; a `Revealed` is queued when it ends.
    pub fn reveal_tick(&self) {
        let now = self.sh.now();
        self.sh.reveal_step(|st| reveal::on_tick(st, now));
    }

    /// Shows the terminal now.
    pub fn finish_reveal(&self) {
        self.sh.finish_reveal();
    }

    /// Starts the shell on a worker thread (the spawn blocks).
    pub fn start(self: &Arc<Self>, opts: StartOptions) {
        // On the calling thread, BEFORE the spawn worker: no chunk can arrive first.
        if let Some(r) = flare_agent::resume::valid_resume(opts.resume.as_deref()) {
            let st = reveal::start_reveal(reveal::agent_of_resume(r), self.sh.now());
            *lk(&self.sh.reveal) = Some(st);
        }
        let sh = Arc::clone(&self.sh);
        // Claimed HERE, before the worker exists: a tab closed in between still finds the id and its
        // kill is remembered, so the worker starts no shell nobody can reach (review #32).
        let reserved = sh.mgr.reserve(&sh.id);
        let r = std::thread::Builder::new()
            .name("flare-start".into())
            .spawn(move || {
                let sink: Arc<dyn SessionSink> = Arc::new(Sink(Arc::downgrade(&sh)));
                let ok = reserved
                    && match opts.remote {
                        Some(remote) => sh.mgr.spawn_remote_reserved(&sh.id, remote, sink),
                        None => sh.mgr.spawn_reserved(
                            &sh.id,
                            &opts.root,
                            opts.shell_id.as_deref(),
                            sink,
                            opts.resume.as_deref(),
                            opts.plugin.as_ref(),
                        ),
                    };
                if ok {
                    lk(&sh.events).push(TermEvent::Started);
                    // After a spawn resolves, the real size is sent once more.
                    let (c, r) = {
                        let e = sh.emu.lock();
                        (e.cols(), e.rows())
                    };
                    sh.mgr.resize(&sh.id, c, r);
                } else {
                    sh.finish_reveal();
                    let evs = sh
                        .emu
                        .lock()
                        .feed(b"\x1b[31mCould not start the shell.\x1b[0m\r\n");
                    sh.handle(evs);
                    lk(&sh.events).push(TermEvent::StartFailed);
                }
                sh.wake();
            });
        if let Err(e) = r {
            log::warn!("could not start the spawn worker: {e}");
            if reserved {
                self.sh.mgr.release(&self.sh.id);
            }
            self.sh.finish_reveal();
            lk(&self.sh.events).push(TermEvent::StartFailed);
            self.sh.wake();
        }
    }

    /// The viewport as a renderer Grid; clears the dirty flag.
    pub fn snapshot(&self) -> flare_render::grid::Grid {
        self.sh.dirty.store(false, Ordering::SeqCst);
        self.emu.lock().snapshot()
    }

    /// [`TermHandle::snapshot`] into a grid the caller keeps and refills each frame (M10 Task 67):
    /// the lock is held only for the copy of the visible rows, which allocates nothing once the
    /// buffer has the viewport's size. The wait for the lock and the copy go to the frame trace.
    pub fn snapshot_into(&self, out: &mut flare_render::grid::Grid) {
        self.sh.dirty.store(false, Ordering::SeqCst);
        let trace = flare_render::stats::FrameTrace::global();
        if !trace.on() {
            self.emu.lock().snapshot_into(out);
            return;
        }
        use flare_render::stats::TracePhase;
        let asked = Instant::now();
        let emu = self.emu.lock();
        let held = Instant::now();
        emu.snapshot_into(out);
        drop(emu);
        trace.add(TracePhase::LockWait, held.duration_since(asked));
        trace.add(TracePhase::Snapshot, held.elapsed());
    }

    pub fn is_dirty(&self) -> bool {
        self.sh.dirty.load(Ordering::SeqCst)
    }

    /// Resizes the emulator, then the pty; the session's helper flushes what is already read first, so
    /// ConPTY's answer lands on the new size and the UI thread never parses a backlog (rule 34).
    pub fn resize(&self, cols: usize, rows: usize) {
        if cols < 2 || rows < 1 {
            return;
        }
        self.emu.lock().resize(cols, rows);
        self.sh.mgr.resize(&self.sh.id, cols, rows);
        self.sh.dirty.store(true, Ordering::SeqCst);
        lk(&self.sh.events).push(TermEvent::Resized);
    }

    pub fn scroll_view(&self, delta_lines: i32) {
        self.emu.lock().scroll_view(delta_lines);
        self.sh.dirty.store(true, Ordering::SeqCst);
    }

    /// `notches` > 0 is wheel up. Never typing, never snaps the view. True when the view itself
    /// moved: no output follows such a scroll, so the window must ask for the redraw (the GPU
    /// path paints only on a request, M10 Task 70).
    pub fn wheel(&self, notches: i32, col: usize, row: usize) -> bool {
        self.wheel_step(notches, false, col, row)
    }

    /// A macOS trackpad drag in whole lines (`lines` > 0 is up): the [`wheel`](Self::wheel) rules,
    /// one line a line (rule 1129). True when the view itself moved.
    pub fn wheel_lines(&self, lines: i32, col: usize, row: usize) -> bool {
        self.wheel_step(lines, true, col, row)
    }

    fn wheel_step(&self, steps: i32, lines: bool, col: usize, row: usize) -> bool {
        let m = self.mode();
        let ctx = WheelContext {
            mouse: m.mouse,
            sgr_mouse: m.sgr_mouse,
            alt_screen: m.alt_screen,
            alternate_scroll: m.alternate_scroll,
            app_cursor: m.app_cursor,
        };
        let action = if lines {
            crate::wheel::wheel_lines_action(steps, &ctx, col, row)
        } else {
            wheel_action(steps, &ctx, col, row)
        };
        match action {
            WheelAction::ScrollView(n) => {
                self.scroll_view(n);
                true
            }
            WheelAction::Bytes(b) => {
                self.sh.mgr.write(&self.sh.id, &b);
                false
            }
            WheelAction::None => false,
        }
    }

    /// Encodes the key, snaps the view to the bottom, writes, and marks typing.
    pub fn send_key(&self, ev: &KeyEvent) -> bool {
        self.key_by(ev, Typist::Local)
    }

    fn key_by(&self, ev: &KeyEvent, by: Typist) -> bool {
        let app_cursor = self.mode().app_cursor;
        let Some(bytes) = crate::key_encode::encode_key(ev, app_cursor) else {
            return false;
        };
        self.sh.finish_reveal();
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, &bytes);
        self.sh.mark_typed_by(by);
        let data = String::from_utf8_lossy(&bytes);
        if flare_agent::question::answers_question(&data) {
            lk(&self.sh.events).push(TermEvent::AnswerKey(data.into_owned()));
        }
        true
    }

    /// IME commit or dictation text: sent as is, counts as typing.
    pub fn send_text(&self, text: &str) {
        self.text_by(text, Typist::Local);
    }

    fn text_by(&self, text: &str, by: Typist) {
        if text.is_empty() {
            return;
        }
        self.sh.finish_reveal();
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, text.as_bytes());
        self.sh.mark_typed_by(by);
    }

    /// A paste the caller already sanitised; bracketed only when the program asked.
    pub fn paste(&self, text: &str) {
        self.write_paste(text, self.mode().bracketed_paste);
    }

    /// A sanitised paste that must wait at the prompt for the person to send ("Ask agent", #33). Bracketed when the
    /// program asked; when it did not, a CR inside the text would submit that line, so the text goes as one line and
    /// no CR is written at all.
    pub fn paste_unsent(&self, text: &str) {
        let bracketed = self.mode().bracketed_paste;
        if bracketed {
            self.write_paste(text, true);
        } else {
            self.write_paste(&flare_shells::paste::one_line(text), false);
        }
    }

    fn write_paste(&self, text: &str, bracketed: bool) {
        self.paste_by(text, bracketed, Typist::Local);
    }

    fn paste_by(&self, text: &str, bracketed: bool, by: Typist) {
        if text.is_empty() {
            return;
        }
        let bytes = flare_shells::paste::paste_bytes(text, bracketed);
        self.sh.finish_reveal();
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, &bytes);
        self.sh.mark_typed_by(by);
    }

    /// A remote viewer's typing (remote sessions PR 2, rule 279): the second deliberate exception to rule 2 beside
    /// `plugin_input`, called only by the host's `remote.input` handler. Each intent goes the way the same thing typed
    /// here goes: a key through `encode_key` with this tab's modes (and its answer key, so a question is answered as a
    /// local key answers it), a text as committed text, a paste sanitised again (rule 1) and bracketed when the program
    /// asked. A text with a control character is dropped (the link checked it already). Counts as typing by the remote
    /// typist. Returns how many intents wrote something.
    pub fn remote_input(&self, intents: &[RemoteIntent]) -> usize {
        let mut wrote = 0;
        for i in intents {
            let ok = match i {
                RemoteIntent::Key(ev) => self.key_by(ev, Typist::Remote),
                RemoteIntent::Text(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                    self.text_by(t, Typist::Remote);
                    true
                }
                RemoteIntent::Text(_) => false,
                RemoteIntent::Paste(p) => {
                    let clean = flare_shells::paste::sanitize_paste(p);
                    let ok = !clean.is_empty();
                    if ok {
                        self.paste_by(&clean, self.mode().bracketed_paste, Typist::Remote);
                    }
                    ok
                }
            };
            wrote += usize::from(ok);
        }
        wrote
    }

    /// Who typed in this tab last, or None before anyone did (the size owner of rule 282).
    pub fn last_typist(&self) -> Option<Typist> {
        match self.sh.typist.load(Ordering::SeqCst) {
            TYPIST_LOCAL => Some(Typist::Local),
            TYPIST_REMOTE => Some(Typist::Remote),
            _ => None,
        }
    }

    /// The tab's screen and up to `lines` lines of history above it as text, with the number the session's output tail
    /// had reached when it was read (`remote.snapshot`). Read while no batch is being parsed, so the text holds every
    /// byte before `seq` and none after. None when the session has no tail. Waits for one batch: a control channel
    /// worker's call, never the UI thread's (rule 34). Colours and styles are not kept: it is the text a reconnect shows.
    /// An open synchronized update (DEC 2026) is let go first: its bytes are before `seq` but not yet on the grid, so
    /// without that the viewer would never get them (rule 285).
    pub fn remote_snapshot(&self, lines: usize) -> Option<RemoteSnapshot> {
        self.sh.mgr.with_tail_still(&self.sh.id, |seq| {
            let mut e = self.emu.lock();
            let evs = e.release_sync();
            let text = e.text_tail(lines.max(1));
            let (cols, rows) = (e.cols(), e.rows());
            drop(e);
            self.sh.handle(evs);
            let mut data = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
            data.extend_from_slice(text.join("\r\n").as_bytes());
            RemoteSnapshot {
                seq,
                cols,
                rows,
                data,
            }
        })
    }

    /// A plugin's typing (plugins phase 5, rule 2's plugin exception): the one way a caller outside Flare's own
    /// interface writes into a shell. Only the app's `tabs.input` path calls it, for a plugin that holds `input` the
    /// person approved, on a tab the plugin may see; the agent has no way here (rule 188). A paste is sanitised again
    /// and goes as the person's paste does (bracketed when the program asked); an Enter is one carriage return. False
    /// when there was nothing to write.
    pub fn plugin_input(&self, input: PluginInput<'_>) -> bool {
        match input {
            PluginInput::Paste(text) => {
                let text = flare_shells::paste::sanitize_paste(text);
                if text.is_empty() {
                    return false;
                }
                self.paste(&text);
            }
            PluginInput::Enter => {
                self.sh.finish_reveal();
                self.sh.snap_to_bottom();
                self.sh.mgr.write(&self.sh.id, b"\r");
                self.sh.mark_typed();
            }
        }
        true
    }

    /// A report the program asked for (a mouse report): written as is. Not typing, no snap (D11).
    pub fn write_report(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.sh.mgr.write(&self.sh.id, bytes);
    }

    /// What the program asked of theme reports (?2031h) and the mode it was last told.
    pub fn theme_reports(&self) -> flare_theme::reports::ThemeReports {
        self.emu.lock().theme_reports()
    }

    pub fn set_theme_reports(&self, s: flare_theme::reports::ThemeReports) {
        self.emu.lock().set_theme_reports(s);
    }

    /// The colours OSC 10 and 11 queries are answered with (the theme the tab wears).
    pub fn set_colours(&self, fg: &str, bg: &str) {
        self.emu.lock().set_colours(fg, bg);
    }

    /// Whether the colour queries are already answered with these.
    pub fn colours_are(&self, fg: &str, bg: &str) -> bool {
        self.emu.lock().colours_are(fg, bg)
    }

    /// The size in columns and rows.
    pub fn size(&self) -> (usize, usize) {
        let e = self.emu.lock();
        (e.cols(), e.rows())
    }

    /// The id the RUNNING shell was spawned with, resolved by the session even when the tab asked
    /// for none; None before the spawn finished or after it failed.
    pub fn shell_id(&self) -> Option<String> {
        self.sh.mgr.shell_of(&self.sh.id)
    }

    /// Moves the shell to a folder. Not typing.
    pub fn cd(&self, path: &str) -> bool {
        self.sh.mgr.cd(&self.sh.id, path)
    }

    pub fn drain_events(&self) -> Vec<TermEvent> {
        let mut evs = lk(&self.sh.events);
        self.sh.output_pending.store(false, Ordering::SeqCst);
        std::mem::take(&mut *evs)
    }

    /// The last non-blank line of the buffer and the lines above it (see `Emulator::text_tail`).
    pub fn screen_tail(&self, rows: usize) -> Vec<String> {
        self.emu.lock().text_tail(rows)
    }

    /// `screen_tail` with dim, reverse-video and grey cells blanked: the typed ink only (a placeholder in an
    /// empty prompt box is not a draft).
    pub fn typed_tail(&self, rows: usize) -> Vec<String> {
        self.emu.lock().typed_tail(rows)
    }

    /// The question watch's rows: the live screen and a screen of history above it.
    pub fn question_rows(&self) -> Vec<String> {
        self.emu.lock().watch_rows()
    }

    /// The local dev server address the program printed since the screen was last cleared.
    pub fn dev_server(&self) -> crate::dev_server::DevState {
        self.emu.lock().dev_state()
    }

    /// Output arrived that `dev_server` has not read yet.
    pub fn dev_pending(&self) -> bool {
        self.emu.lock().dev_pending()
    }

    pub fn last_typed(&self) -> Option<Instant> {
        *lk(&self.sh.last_typed)
    }

    pub fn mode(&self) -> ModeSnapshot {
        self.emu.lock().mode()
    }

    /// Runs `f` on the emulator, then marks the view dirty and wakes the UI.
    pub fn with_emu<R>(&self, f: impl FnOnce(&mut Emulator) -> R) -> R {
        let r = f(&mut self.emu.lock());
        self.sh.wake();
        r
    }

    /// The cursor stops since the last call, oldest first (the IME caret hold reads them each frame).
    pub fn take_cursor_stops(&self) -> Vec<CursorStop> {
        std::mem::take(&mut *lk(&self.sh.cursor_log))
    }

    /// Bytes this tab's scrollback holds (see `Emulator::scrollback_bytes`).
    pub fn scrollback_bytes(&self) -> usize {
        self.emu.lock().scrollback_bytes()
    }

    /// Read only: no dirty flag, no wake.
    pub fn with_emu_quiet<R>(&self, f: impl FnOnce(&Emulator) -> R) -> R {
        f(&self.emu.lock())
    }

    /// Reads the selection on a worker, `CHUNK_ROWS` rows per hold of the emulator lock, so the UI's
    /// snapshot and the pty reader wait at most one chunk. `done(Some(text))` carries the text
    /// the selection covers when the read ends; `done(None)` when the selection changed meanwhile
    /// (nothing is copied). The selection's generation is taken here, before the worker starts.
    pub fn selection_text_async(&self, done: Box<dyn FnOnce(Option<String>) + Send>) {
        let emu = Arc::clone(&self.emu);
        let generation = emu.lock().sel_generation();
        std::thread::spawn(move || {
            let mut out = String::new();
            let mut from = 0usize;
            let text = loop {
                let g = emu.lock();
                if g.sel_generation() != generation {
                    break None;
                }
                let total = g.sel_row_count();
                if from >= total {
                    break Some(out);
                }
                let (piece, _) = g.sel_text_rows(from..(from + CHUNK_ROWS).min(total));
                drop(g);
                out.push_str(&piece);
                from += CHUNK_ROWS;
            };
            done(text);
        });
    }

    /// One frame's decoration (Task 16d). Locks the emulator (the link scan may run) and does not
    /// mark the view dirty; a frame calls it right after `snapshot`.
    pub fn decor(&self, req: &mut DecorReq) -> Decor {
        self.emu.lock().decor(req)
    }

    pub fn kill(&self) {
        self.sh.mgr.kill(&self.sh.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeShells, FakeSpawner, FakeState};
    use crate::sel_machine::{Anchor, SelOut, SelSpec};
    use crate::session::SessionOptions;
    use flare_render::color::Color;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    struct T {
        h: Arc<TermHandle>,
        sp: Arc<FakeSpawner>,
    }

    fn start_opts() -> StartOptions {
        StartOptions {
            root: "C:\\".into(),
            ..Default::default()
        }
    }

    impl T {
        fn new() -> T {
            let sp = FakeSpawner::new();
            let mgr = SessionManager::new(sp.clone(), FakeShells::new(), SessionOptions::default());
            T {
                h: TermHandle::new("a", mgr, 80, 24),
                sp,
            }
        }
        fn has(&self, ev: &TermEvent) -> bool {
            lk(&self.h.sh.events).contains(ev)
        }
        fn started() -> T {
            let t = T::new();
            t.h.start(start_opts());
            wait_until("the spawn", || !t.sp.made().is_empty());
            wait_until("Started", || t.has(&TermEvent::Started));
            t
        }
        fn fake(&self) -> Arc<FakeState> {
            self.sp.made()[0].clone()
        }
        fn row(&self, r: usize) -> String {
            let g = self.h.snapshot();
            (0..g.cols())
                .map(|c| g.cell(c, r).ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        }
        fn row0(&self) -> String {
            self.row(0)
        }
    }

    fn key_a() -> KeyEvent {
        KeyEvent::new("a", "KeyA").text("a")
    }

    // Remote sessions PR 2: typing from a remote viewer, the output tail and the snapshot.

    /// Rule 279: a remote viewer's intents go the typed way: a key through the encoder, a text as committed, a paste
    /// sanitised (the escape that would end a bracketed paste is gone); a text with a control character is dropped.
    #[test]
    fn remote_input_types_keys_text_and_a_sanitised_paste() {
        let t = T::started();
        let n = t.h.remote_input(&[
            RemoteIntent::Text("echo hi".into()),
            RemoteIntent::Key(KeyEvent::new("Enter", "Enter")),
            RemoteIntent::Key(KeyEvent::new("c", "KeyC").text("c").ctrl()),
            RemoteIntent::Paste("a\x1b[201~b".into()),
            RemoteIntent::Text("bad\r".into()),
            RemoteIntent::Paste("\x1b".into()),
        ]);
        assert_eq!(n, 4);
        wait_until("the writes", || t.fake().written().len() >= 16);
        let w = String::from_utf8_lossy(&t.fake().written()).into_owned();
        assert_eq!(w, "echo hi\r\u{3}a[201~b");
        assert_eq!(t.h.last_typist(), Some(Typist::Remote));
        assert!(t.has(&TermEvent::Typed));
        // A local key after it makes this machine the last typist again.
        assert!(t.h.send_key(&key_a()));
        assert_eq!(t.h.last_typist(), Some(Typist::Local));
    }

    #[test]
    fn nobody_is_the_typist_before_anyone_types() {
        let t = T::started();
        assert_eq!(t.h.last_typist(), None);
        t.h.send_text("x");
        assert_eq!(t.h.last_typist(), Some(Typist::Local));
    }

    /// Rule 281: the tail numbers what the tab printed after the follow began, in the order the tab saw it, and a
    /// snapshot taken between two batches holds the bytes before its number and none after.
    #[test]
    fn the_output_tail_follows_the_tab_and_a_snapshot_meets_it_without_a_byte_twice() {
        let t = T::started();
        let mgr = Arc::clone(&t.h.sh.mgr);
        t.fake().feed(b"before\r\n");
        wait_until("before", || t.text().contains("before"));
        let tail = mgr
            .subscribe_tail("a")
            .expect("a running session has a tail");
        assert!(
            tail.same(&mgr.subscribe_tail("a").unwrap()),
            "one tail a tab"
        );
        let start = tail.end();
        t.fake().feed(b"one\r\n");
        wait_until("one in the tail", || tail.end() > start);
        let snap = t.h.remote_snapshot(100).expect("a snapshot");
        assert!(String::from_utf8_lossy(&snap.data).contains("one"));
        assert_eq!(snap.seq, tail.end());
        assert_eq!((snap.cols, snap.rows), (80, 24));
        t.fake().feed(b"two\r\n");
        wait_until("two in the tail", || tail.end() > snap.seq);
        let after = tail.read(snap.seq, 1 << 20);
        assert_eq!(after.data, b"two\r\n");
        assert!(!after.gap);
        // From the follow's start: exactly what came since, once.
        let all = tail.read(start, 1 << 20);
        assert_eq!(all.data, b"one\r\ntwo\r\n");
        assert!(mgr.subscribe_tail("nobody").is_none());
    }

    /// Rule 285: a snapshot taken while a synchronized update (DEC 2026) is still open holds the bytes the update kept
    /// back, so the viewer that polls from its `seq` loses no part of the frame.
    #[test]
    fn a_snapshot_inside_an_open_synchronized_update_holds_the_bytes_it_kept_back() {
        let t = T::started();
        let mgr = Arc::clone(&t.h.sh.mgr);
        let tail = mgr.subscribe_tail("a").expect("a tail");
        let start = tail.end();
        t.fake().feed(b"\x1b[?2026hhalf a frame");
        wait_until("the first half in the tail", || tail.end() > start);
        let snap = t.h.remote_snapshot(100).expect("a snapshot");
        assert_eq!(snap.seq, tail.end());
        assert!(
            String::from_utf8_lossy(&snap.data).contains("half a frame"),
            "the snapshot lost the bytes the update kept back"
        );
        assert_eq!(t.h.emu.lock().sync_deadline(), None);
        t.fake().feed(b" and the rest\x1b[?2026l");
        wait_until("the rest", || t.text().contains("and the rest"));
        assert_eq!(
            tail.read(snap.seq, 1 << 20).data,
            b" and the rest\x1b[?2026l"
        );
    }

    // The resume reveal.

    const RID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    struct R {
        t: T,
        now: Arc<std::sync::atomic::AtomicU64>,
    }

    impl R {
        fn at(&self, ms: u64) {
            self.now.store(ms, Ordering::SeqCst);
        }
        fn tick(&self, ms: u64) {
            self.at(ms);
            self.t.h.reveal_tick();
        }
        fn feed(&self, bytes: &[u8]) {
            self.t.fake().feed(bytes);
        }
        /// Feeds a marker line and waits for it, so everything before it is parsed.
        fn sync(&self, marker: &str) {
            self.feed(marker.as_bytes());
            let m = marker.to_string();
            wait_until("the marker", || self.t.text().contains(&m));
        }
        fn revealed_events(&self) -> usize {
            self.t
                .h
                .drain_events()
                .iter()
                .filter(|e| **e == TermEvent::Revealed)
                .count()
        }
    }

    impl T {
        fn text(&self) -> String {
            let g = self.h.snapshot();
            (0..g.rows())
                .map(|r| {
                    (0..g.cols())
                        .map(|c| g.cell(c, r).ch)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    fn resumed_with(resume: Option<&str>) -> R {
        let t = T::new();
        let now = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let n = Arc::clone(&now);
        t.h.set_clock(Arc::new(move || n.load(Ordering::SeqCst)));
        t.h.start(StartOptions {
            resume: resume.map(str::to_string),
            ..start_opts()
        });
        wait_until("the spawn", || !t.sp.made().is_empty());
        wait_until("Started", || t.has(&TermEvent::Started));
        R { t, now }
    }

    fn resumed() -> R {
        resumed_with(Some(RID))
    }

    #[test]
    fn a_resume_never_writes_into_the_shell() {
        let r = resumed();
        r.feed(b"PS C:\\work> ");
        r.sync("one");
        r.feed(b"\x1b]0;claude\x07");
        r.sync("two");
        assert!(r.t.fake().written().is_empty());
    }

    #[test]
    fn the_clear_alone_makes_no_reply() {
        let mut e = Emulator::new(80, 24);
        assert!(e.feed(reveal::CLEAR).is_empty());
    }

    #[test]
    fn a_resume_clears_the_grid_just_before_the_agents_title() {
        let r = resumed();
        r.at(3600);
        r.feed(b"PS C:\\work> \x1b]0;claude\x07agent text");
        wait_until("agent text", || r.t.text().contains("agent text"));
        assert!(!r.t.text().contains("PS C:"));
        assert!(r.t.fake().written().is_empty());
    }

    #[test]
    fn the_clear_is_written_once() {
        let r = resumed();
        r.feed(b"\x1b]0;claude\x07first");
        wait_until("first", || r.t.text().contains("first"));
        r.feed(b"typed\x1b]0;claude\x07");
        r.sync("after");
        assert!(r.t.text().contains("typed"));
    }

    #[test]
    fn a_shell_title_or_a_folder_called_claude_clears_nothing() {
        let r = resumed();
        r.feed(b"prompt one\x1b]0;C:\\Users\\me\\Documents\\Claude\x07");
        r.feed(b"\x1b]0;pwsh\x07");
        r.sync(" end");
        assert!(r.t.text().contains("prompt one"));
    }

    #[test]
    fn the_terminal_is_revealed_150_ms_after_the_agents_own_title() {
        let r = resumed();
        r.at(3600);
        r.feed(b"\x1b]0;claude\x07");
        r.sync("x");
        r.at(5410);
        r.feed("\x1b]0;\u{2733} Claude Code\x07".as_bytes());
        r.sync("y");
        r.tick(5559);
        assert!(r.t.h.resuming());
        r.tick(5560);
        assert!(!r.t.h.resuming());
        r.tick(9000);
        assert_eq!(r.revealed_events(), 1);
    }

    #[test]
    fn the_alternate_screen_reveals_too() {
        let r = resumed();
        r.at(3600);
        r.feed(b"\x1b]0;claude\x07");
        r.sync("x");
        r.at(5445);
        r.feed(b"\x1b[?1049h");
        wait_until("the alt screen", || r.t.h.mode().alt_screen);
        r.tick(5594);
        assert!(r.t.h.resuming());
        r.tick(5595);
        assert!(!r.t.h.resuming());
    }

    #[test]
    fn a_key_a_paste_an_exit_and_a_failed_spawn_reveal_at_once() {
        let r = resumed();
        assert!(r.t.h.resuming());
        r.t.h.send_key(&key_a());
        assert!(!r.t.h.resuming());
        let r = resumed();
        r.t.h.paste("hi");
        assert!(!r.t.h.resuming());
        let r = resumed();
        r.t.fake().exit(0);
        wait_until("the reveal", || !r.t.h.resuming());
        let t = T::new();
        t.sp.fail_next_spawn();
        t.h.start(StartOptions {
            resume: Some(RID.into()),
            ..start_opts()
        });
        wait_until("StartFailed", || t.has(&TermEvent::StartFailed));
        assert!(!t.h.resuming());
    }

    #[test]
    fn the_ceiling_reveals_whatever_is_there() {
        let r = resumed();
        r.tick(11_999);
        assert!(r.t.h.resuming());
        r.tick(12_000);
        assert!(!r.t.h.resuming());
    }

    #[test]
    fn a_plain_tab_is_never_resuming_or_cleared() {
        let r = resumed_with(None);
        assert!(!r.t.h.resuming());
        r.feed(b"keep me\x1b]0;claude\x07");
        r.sync(" end");
        assert!(r.t.text().contains("keep me"));
        r.tick(20_000);
        assert_eq!(r.revealed_events(), 0);
    }

    #[test]
    fn a_title_split_across_two_chunks_does_not_clear_but_the_ceiling_still_reveals() {
        let r = resumed();
        r.sync("shell text");
        r.feed(b"\x1b]0;cla");
        r.sync(" ");
        r.feed(b"ude\x07");
        r.sync("z");
        assert!(r.t.text().contains("shell text"));
        r.tick(12_000);
        assert!(!r.t.h.resuming());
    }

    #[test]
    fn the_failed_spawn_line_still_shows() {
        let t = T::new();
        t.sp.fail_next_spawn();
        t.h.start(StartOptions {
            resume: Some(RID.into()),
            ..start_opts()
        });
        wait_until("StartFailed", || t.has(&TermEvent::StartFailed));
        assert!(t.row0().starts_with("Could not start the shell."));
    }

    #[test]
    fn start_spawns_and_output_reaches_the_grid() {
        let t = T::new();
        t.h.start(start_opts());
        wait_until("the spawn", || !t.sp.made().is_empty());
        t.fake().feed(b"hello\r\n");
        wait_until("hello", || t.row0() == "hello");
        wait_until("Started", || t.has(&TermEvent::Started));
        assert!(t.h.drain_events().contains(&TermEvent::Started));
    }

    #[test]
    fn a_failed_spawn_writes_the_red_line() {
        let t = T::new();
        t.sp.fail_next_spawn();
        t.h.start(start_opts());
        wait_until("StartFailed", || t.has(&TermEvent::StartFailed));
        assert!(t.row0().starts_with("Could not start the shell."));
        assert_eq!(t.h.snapshot().cell(0, 0).fg, Color::Indexed(1));
    }

    #[test]
    fn typing_marks_last_typed_and_replies_do_not() {
        let t = T::started();
        assert!(t.h.last_typed().is_none());
        t.fake().feed(b"\x1b[5n");
        wait_until("the reply", || t.fake().written() == b"\x1b[0n");
        assert!(t.h.last_typed().is_none());
        assert!(t.h.send_key(&key_a()));
        wait_until("the key", || t.fake().written().ends_with(b"a"));
        assert!(t.h.last_typed().is_some());
    }

    #[test]
    fn every_parsed_slice_leaves_a_cursor_stop() {
        let t = T::started();
        let _ = t.h.take_cursor_stops();
        t.fake().feed(b"\x1b[10;3Hinput");
        wait_until("the first slice", || t.text().contains("input"));
        t.fake().feed(b"\x1b[2;1Hstream");
        wait_until("the second slice", || t.text().contains("stream"));
        let stops = t.h.take_cursor_stops();
        assert!(stops.iter().any(|s| s.line == 9), "{stops:?}");
        assert_eq!(stops.last().map(|s| s.line), Some(1), "{stops:?}");
        assert!(t.h.take_cursor_stops().is_empty());
    }

    #[test]
    fn cd_is_not_typing() {
        let t = T::started();
        assert!(t.h.cd("C:\\x"));
        wait_until("the cd line", || {
            String::from_utf8_lossy(&t.fake().written())
                .contains("Set-Location -LiteralPath 'C:\\x'\r")
        });
        assert!(t.h.last_typed().is_none());
    }

    fn feed_lines(t: &T, n: usize) {
        let mut s = String::new();
        for i in 0..n {
            s.push_str(&format!("line{i}\r\n"));
        }
        t.fake().feed(s.as_bytes());
        let last = format!("line{}", n - 1);
        wait_until("the lines", || {
            let e = t.h.emu.lock();
            (0..e.rows() as i32).any(|r| e.row_text(r) == last)
        });
    }

    #[test]
    fn a_key_snaps_the_view_back_to_the_bottom() {
        let t = T::started();
        feed_lines(&t, 100);
        t.h.wheel(2, 0, 0);
        assert!(t.h.emu.lock().display_offset() > 0);
        t.h.send_key(&key_a());
        assert_eq!(t.h.emu.lock().display_offset(), 0);
    }

    #[test]
    fn an_open_synchronized_update_still_shows() {
        let t = T::started();
        t.fake().feed(b"\x1b[?2026hhi");
        wait_until("hi", || t.row0() == "hi");
    }

    #[test]
    fn resize_before_the_shell_exists_is_applied_at_birth() {
        let t = T::new();
        t.h.resize(100, 30);
        t.h.start(start_opts());
        wait_until("the spawn", || !t.sp.made().is_empty());
        let s = t.fake().spec();
        assert_eq!((s.cols, s.rows), (100, 30));
    }

    #[test]
    fn resize_updates_the_emulator_and_the_pty() {
        let t = T::started();
        t.h.resize(90, 20);
        let g = t.h.snapshot();
        assert_eq!((g.cols(), g.rows()), (90, 20));
        wait_until("the pty size", || t.fake().size() == (90, 20));
    }

    #[test]
    fn the_wheel_scrolls_the_normal_screen_and_sends_arrows_on_the_alt_screen() {
        let t = T::started();
        feed_lines(&t, 100);
        t.h.wheel(1, 0, 0);
        assert_eq!(t.h.emu.lock().display_offset(), 3);
        t.fake().feed(b"\x1b[?1049h");
        wait_until("the alt screen", || t.h.mode().alt_screen);
        t.h.wheel(1, 0, 0);
        wait_until("arrows", || {
            t.fake().written().ends_with(b"\x1b[A\x1b[A\x1b[A")
        });
        assert!(t.h.last_typed().is_none());
    }

    #[test]
    fn a_wheel_says_when_the_view_moved_so_the_window_can_redraw() {
        let t = T::started();
        feed_lines(&t, 100);
        assert!(t.h.wheel(1, 0, 0), "a scroll of the view must be reported");
        t.fake().feed(b"\x1b[?1049h");
        wait_until("the alt screen", || t.h.mode().alt_screen);
        assert!(
            !t.h.wheel(1, 0, 0),
            "arrows sent to the program are not a view change"
        );
    }

    #[test]
    fn a_paste_is_bracketed_only_when_asked() {
        let t = T::started();
        t.h.paste("a\nb");
        wait_until("plain", || t.fake().written() == b"a\rb");
        assert!(t.h.last_typed().is_some());
        t.fake().feed(b"\x1b[?2004h");
        wait_until("mode", || t.h.mode().bracketed_paste);
        t.h.paste("a\nb");
        wait_until("bracketed", || {
            t.fake().written().ends_with(b"\x1b[200~a\rb\x1b[201~")
        });
    }

    #[test]
    fn output_wakes_the_ui() {
        let t = T::started();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        t.h.set_waker(Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        }));
        let _ = t.h.snapshot();
        assert!(!t.h.is_dirty());
        t.fake().feed(b"x");
        wait_until("a wake", || n.load(Ordering::SeqCst) >= 1);
        assert!(t.h.is_dirty());
        let _ = t.h.snapshot();
        assert!(!t.h.is_dirty());
    }

    /// Fills the history with `n` numbered rows and selects all of them.
    fn select_all_rows(t: &T, n: usize) {
        t.h.with_emu(|e| {
            for i in 0..n {
                e.feed(format!("row {i:05}\r\n").as_bytes());
            }
            let total = e.history_size() + e.rows();
            e.sel_apply(SelOut::Set(SelSpec {
                block: false,
                start: Anchor { row: 0, b: 0 },
                end: Anchor {
                    row: total - 1,
                    b: 80,
                },
            }));
        });
    }

    fn read_async(t: &T) -> Option<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        t.h.selection_text_async(Box::new(move |text| {
            let _ = tx.send(text);
        }));
        rx.recv_timeout(Duration::from_secs(20)).expect("done")
    }

    #[test]
    fn a_big_copy_delivers_the_exact_text_later() {
        let t = T::started();
        select_all_rows(&t, 600);
        let want = t.h.with_emu_quiet(|e| e.sel_text());
        assert_eq!(read_async(&t), Some(want));
    }

    #[test]
    fn a_selection_changed_mid_read_copies_nothing() {
        // 10,000 rows take many chunks; the change lands while the worker is still reading.
        let t = T::started();
        select_all_rows(&t, 10_000);
        let (tx, rx) = std::sync::mpsc::channel();
        t.h.selection_text_async(Box::new(move |text| {
            let _ = tx.send(text);
        }));
        t.h.with_emu(|e| e.sel_clear());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(20)).expect("done"),
            None
        );
    }

    #[test]
    #[ignore = "release timing"]
    fn the_ui_snapshot_never_waits_more_than_5_ms_for_a_copy() {
        let t = T::started();
        select_all_rows(&t, 10_000);
        let (tx, rx) = std::sync::mpsc::channel();
        t.h.selection_text_async(Box::new(move |text| {
            let _ = tx.send(text);
        }));
        let mut longest = Duration::ZERO;
        let text = loop {
            let started = Instant::now();
            let _ = t.h.snapshot();
            longest = longest.max(started.elapsed());
            if let Ok(text) = rx.try_recv() {
                break text;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(text.is_some());
        assert!(
            longest < Duration::from_millis(5),
            "the longest snapshot wait was {longest:?}"
        );
    }
    #[test]
    fn with_emu_marks_dirty_and_wakes() {
        let t = T::started();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        t.h.set_waker(Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        }));
        let _ = t.h.snapshot();
        let before = n.load(Ordering::SeqCst);
        assert!(!t.h.with_emu_quiet(|e| e.has_selection()));
        assert!(!t.h.is_dirty());
        t.h.with_emu(|e| e.sel_clear());
        assert!(t.h.is_dirty());
        assert_eq!(n.load(Ordering::SeqCst), before + 1);
    }

    #[test]
    fn bells_are_gated_to_one_a_second() {
        let t = T::started();
        for _ in 0..3 {
            t.fake().feed(b"\x07");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        let bells =
            t.h.drain_events()
                .iter()
                .filter(|e| **e == TermEvent::Bell)
                .count();
        assert_eq!(bells, 1);
    }

    #[test]
    fn the_exit_comes_after_the_last_output() {
        let t = T::started();
        t.fake().feed(b"bye");
        t.fake().exit(3);
        wait_until("the exit", || t.has(&TermEvent::Exited(Some(3))));
        assert_eq!(t.row0(), "bye");
        assert_eq!(t.h.drain_events().last(), Some(&TermEvent::Exited(Some(3))));
    }

    #[test]
    fn start_sends_the_real_size_once_more() {
        let t = T::new();
        t.h.resize(100, 30);
        t.h.start(start_opts());
        wait_until("the spawn", || !t.sp.made().is_empty());
        wait_until("Started", || t.has(&TermEvent::Started));
        wait_until("the resize", || t.fake().resize_calls() == 1);
        assert_eq!(t.fake().size(), (100, 30));
    }

    #[test]
    fn output_keeps_being_parsed_while_nothing_looks() {
        let t = T::started();
        t.fake().feed(b"first\r\n");
        let mut s = String::new();
        for i in 0..50 {
            s.push_str(&format!("more{i}\r\n"));
        }
        t.fake().feed(s.as_bytes());
        std::thread::sleep(Duration::from_millis(200));
        let g = t.h.snapshot();
        let text: Vec<String> = (0..g.rows())
            .map(|r| (0..g.cols()).map(|c| g.cell(c, r).ch).collect::<String>())
            .collect();
        assert!(text.iter().any(|l| l.starts_with("more49")));
        assert!(t.h.emu.lock().history_size() > 0);
        assert!(!text.iter().any(|l| l.starts_with("first")));
    }

    #[test]
    fn osc_events_reach_the_app() {
        let t = T::started();
        t.fake()
            .feed(b"\x1b]0;t\x07\x1b]9;9;C:\\w\x1b\\\x1b]52;c;aGk=\x07");
        wait_until("the events", || {
            lk(&t.h.sh.events)
                .iter()
                .any(|e| matches!(e, TermEvent::ClipboardWrite(_)))
        });
        let evs = t.h.drain_events();
        assert!(evs.contains(&TermEvent::Title("t".into())));
        assert!(evs.contains(&TermEvent::Cwd("C:\\w".into())));
        assert!(evs.contains(&TermEvent::ClipboardWrite("hi".into())));
    }

    fn key(k: &str, code: &str) -> KeyEvent {
        KeyEvent::new(k, code)
    }

    #[test]
    fn output_is_one_event_per_drained_batch() {
        let t = T::started();
        t.h.drain_events();
        for _ in 0..5 {
            t.h.sh.emu.lock().feed(b"x");
            t.h.sh.note_output();
        }
        assert_eq!(t.h.drain_events(), vec![TermEvent::Output]);
        t.h.sh.note_output();
        assert_eq!(t.h.drain_events(), vec![TermEvent::Output]);
        t.fake().exit(0);
        wait_until("the exit", || t.has(&TermEvent::Exited(Some(0))));
        assert_eq!(t.h.drain_events(), vec![TermEvent::Exited(Some(0))]);
    }

    #[test]
    fn output_does_not_reorder_a_title() {
        let t = T::started();
        t.h.drain_events();
        t.fake().feed(b"\x1b]0;t1\x07text");
        wait_until("Output", || t.has(&TermEvent::Output));
        assert_eq!(
            t.h.drain_events(),
            vec![TermEvent::Title("t1".into()), TermEvent::Output]
        );
    }

    #[test]
    fn typing_is_one_typed_event_and_the_next_output_queues_after_it() {
        let t = T::started();
        t.h.drain_events();
        t.h.send_key(&key_a());
        t.fake().feed(b"echo");
        wait_until("Output", || t.has(&TermEvent::Output));
        assert_eq!(
            t.h.drain_events(),
            vec![TermEvent::Typed, TermEvent::Output]
        );

        t.fake().feed(b"one");
        wait_until("Output", || t.has(&TermEvent::Output));
        t.h.send_key(&key_a());
        t.fake().feed(b"two");
        wait_until("two Outputs", || {
            lk(&t.h.sh.events)
                .iter()
                .filter(|e| **e == TermEvent::Output)
                .count()
                == 2
        });
        assert_eq!(
            t.h.drain_events(),
            vec![TermEvent::Output, TermEvent::Typed, TermEvent::Output]
        );

        t.h.send_text("x");
        t.h.paste("x");
        assert_eq!(t.h.drain_events(), vec![TermEvent::Typed, TermEvent::Typed]);

        // A terminal reply (a DA answer) is not typing.
        t.fake().feed(b"\x1b[c");
        wait_until("the reply", || {
            String::from_utf8_lossy(&t.fake().written()).contains("\x1b[?")
        });
        wait_until("Output", || t.has(&TermEvent::Output));
        assert_eq!(t.h.drain_events(), vec![TermEvent::Output]);
    }

    #[test]
    fn only_the_answer_keys_are_reported() {
        let t = T::started();
        let answers = [
            (key("Enter", "Enter"), "\r"),
            (key("Escape", "Escape"), "\x1b"),
            (key("c", "KeyC").ctrl(), "\x03"),
            (key("1", "Digit1").text("1"), "1"),
            (key("9", "Digit9").text("9"), "9"),
        ];
        for (k, data) in answers {
            t.h.drain_events();
            assert!(t.h.send_key(&k));
            assert_eq!(
                t.h.drain_events(),
                vec![TermEvent::Typed, TermEvent::AnswerKey(data.into())],
                "{data:?}"
            );
        }
        let others = [
            key("0", "Digit0").text("0"),
            key_a(),
            key("ArrowDown", "ArrowDown"),
            key("Tab", "Tab"),
            key(" ", "Space").text(" "),
        ];
        for k in others {
            t.h.drain_events();
            assert!(t.h.send_key(&k));
            assert_eq!(t.h.drain_events(), vec![TermEvent::Typed], "{}", k.key);
        }
    }

    #[test]
    fn a_resize_is_reported_even_when_the_size_is_equal() {
        let t = T::started();
        t.h.drain_events();
        t.h.resize(80, 24);
        t.h.resize(80, 24);
        assert_eq!(
            t.h.drain_events(),
            vec![TermEvent::Resized, TermEvent::Resized]
        );
        t.h.resize(1, 24);
        assert!(t.h.drain_events().is_empty());
    }

    #[test]
    fn the_handle_reads_through_the_emulator() {
        let t = T::started();
        t.fake().feed(b"one\r\ntwo");
        wait_until("two", || {
            t.h.screen_tail(16).last().map(String::as_str) == Some("two")
        });
        assert_eq!(t.h.screen_tail(16), t.h.with_emu_quiet(|e| e.text_tail(16)));
        assert_eq!(t.h.question_rows(), t.h.with_emu_quiet(|e| e.watch_rows()));
        let feeder = {
            let h = Arc::clone(&t.h);
            std::thread::spawn(move || {
                for i in 0..1000 {
                    h.sh.emu.lock().feed(format!("f{i}\r\n").as_bytes());
                }
            })
        };
        for _ in 0..1000 {
            let _ = t.h.screen_tail(16);
            let _ = t.h.question_rows();
        }
        feeder.join().expect("feeder");
    }

    #[test]
    fn write_report_is_not_typing() {
        let t = T::started();
        feed_lines(&t, 60);
        t.h.scroll_view(5);
        let off = t.h.with_emu_quiet(|e| e.display_offset());
        assert!(off > 0);
        t.h.write_report(b"\x1b[M !!");
        wait_until("the report", || t.fake().written() == b"\x1b[M !!");
        assert!(t.h.last_typed().is_none());
        assert_eq!(t.h.with_emu_quiet(|e| e.display_offset()), off);
    }

    #[test]
    fn paste_uses_the_xterm_newline_rule_and_the_frame() {
        let t = T::started();
        t.fake().feed(b"\x1b[?2004h");
        wait_until("mode", || t.h.mode().bracketed_paste);
        t.h.paste("a\nb");
        wait_until("framed", || t.fake().written() == b"\x1b[200~a\rb\x1b[201~");
        let u = T::started();
        u.h.paste("a\nb");
        wait_until("plain", || u.fake().written() == b"a\rb");
    }

    #[test]
    fn paste_still_marks_typing_and_snaps() {
        let t = T::started();
        feed_lines(&t, 60);
        t.h.scroll_view(5);
        assert!(t.h.with_emu_quiet(|e| e.display_offset()) > 0);
        t.h.paste("x");
        wait_until("the paste", || t.fake().written() == b"x");
        assert!(t.h.last_typed().is_some());
        assert_eq!(t.h.with_emu_quiet(|e| e.display_offset()), 0);
    }

    #[test]
    fn shell_id_is_the_resolved_one() {
        let cmd = flare_shells::list::ShellDef {
            id: "cmd".into(),
            name: "Command Prompt".into(),
            exe: "cmd.exe".into(),
            args: Vec::new(),
        };
        let sp = FakeSpawner::new();
        let mgr = SessionManager::new(
            sp.clone(),
            FakeShells::with(vec![cmd]),
            SessionOptions::default(),
        );
        let h = TermHandle::new("a", mgr, 80, 24);
        assert_eq!(h.shell_id(), None);
        h.start(start_opts());
        wait_until("Started", || lk(&h.sh.events).contains(&TermEvent::Started));
        assert_eq!(h.shell_id(), Some("cmd".to_string()));
    }
}
