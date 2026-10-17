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
use flare_render::decor::Decor;
use flare_shells::env::PluginEnv;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

/// The largest slice of output parsed under one hold of the emulator lock.
const SLICE: usize = 64 * 1024;

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    pub root: String,
    pub shell_id: Option<String>,
    pub resume: Option<String>,
    pub plugin: Option<PluginEnv>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

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
}

impl Shared {
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
                TermEvent::Reply(b) => self.mgr.write(&self.id, &b),
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
        for slice in bytes.chunks(SLICE) {
            let evs = sh.emu.lock().feed(slice);
            sh.handle(evs);
        }
        sh.note_output();
        sh.wake();
        arm_sync(&sh);
    }

    fn exit(&self, _id: &str, code: Option<u32>) {
        let Some(sh) = self.0.upgrade() else { return };
        lk(&sh.events).push(TermEvent::Exited(code));
        sh.wake();
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

    /// Starts the shell on a worker thread (the spawn blocks).
    pub fn start(self: &Arc<Self>, opts: StartOptions) {
        let sh = Arc::clone(&self.sh);
        let r = std::thread::Builder::new()
            .name("flare-start".into())
            .spawn(move || {
                let sink: Arc<dyn SessionSink> = Arc::new(Sink(Arc::downgrade(&sh)));
                let ok = sh.mgr.spawn(
                    &sh.id,
                    &opts.root,
                    opts.shell_id.as_deref(),
                    sink,
                    opts.resume.as_deref(),
                    opts.plugin.as_ref(),
                );
                if ok {
                    lk(&sh.events).push(TermEvent::Started);
                    // After a spawn resolves, the real size is sent once more.
                    let (c, r) = {
                        let e = sh.emu.lock();
                        (e.cols(), e.rows())
                    };
                    sh.mgr.resize(&sh.id, c, r);
                } else {
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
            lk(&self.sh.events).push(TermEvent::StartFailed);
            self.sh.wake();
        }
    }

    /// The viewport as a renderer Grid; clears the dirty flag.
    pub fn snapshot(&self) -> flare_render::grid::Grid {
        self.sh.dirty.store(false, Ordering::SeqCst);
        self.emu.lock().snapshot()
    }

    pub fn is_dirty(&self) -> bool {
        self.sh.dirty.load(Ordering::SeqCst)
    }

    /// Parses what is already read, resizes the emulator, THEN the pty, so ConPTY's answer lands
    /// on the new size.
    pub fn resize(&self, cols: usize, rows: usize) {
        if cols < 2 || rows < 1 {
            return;
        }
        self.sh.mgr.flush_output(&self.sh.id);
        self.emu.lock().resize(cols, rows);
        self.sh.mgr.resize(&self.sh.id, cols, rows);
        self.sh.dirty.store(true, Ordering::SeqCst);
        lk(&self.sh.events).push(TermEvent::Resized);
    }

    pub fn scroll_view(&self, delta_lines: i32) {
        self.emu.lock().scroll_view(delta_lines);
        self.sh.dirty.store(true, Ordering::SeqCst);
    }

    /// `notches` > 0 is wheel up. Never typing, never snaps the view.
    pub fn wheel(&self, notches: i32, col: usize, row: usize) {
        let m = self.mode();
        let ctx = WheelContext {
            mouse: m.mouse,
            sgr_mouse: m.sgr_mouse,
            alt_screen: m.alt_screen,
            alternate_scroll: m.alternate_scroll,
            app_cursor: m.app_cursor,
        };
        match wheel_action(notches, &ctx, col, row) {
            WheelAction::ScrollView(n) => self.scroll_view(n),
            WheelAction::Bytes(b) => self.sh.mgr.write(&self.sh.id, &b),
            WheelAction::None => {}
        }
    }

    /// Encodes the key, snaps the view to the bottom, writes, and marks typing.
    pub fn send_key(&self, ev: &KeyEvent) -> bool {
        let app_cursor = self.mode().app_cursor;
        let Some(bytes) = crate::key_encode::encode_key(ev, app_cursor) else {
            return false;
        };
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, &bytes);
        self.sh.mark_typed();
        let data = String::from_utf8_lossy(&bytes);
        if flare_agent::question::answers_question(&data) {
            lk(&self.sh.events).push(TermEvent::AnswerKey(data.into_owned()));
        }
        true
    }

    /// IME commit or dictation text: sent as is, counts as typing.
    pub fn send_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, text.as_bytes());
        self.sh.mark_typed();
    }

    /// A paste the caller already sanitised; bracketed only when the program asked.
    pub fn paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = flare_shells::paste::paste_bytes(text, self.mode().bracketed_paste);
        self.sh.snap_to_bottom();
        self.sh.mgr.write(&self.sh.id, &bytes);
        self.sh.mark_typed();
    }

    /// A report the program asked for (a mouse report): written as is. Not typing, no snap (D11).
    pub fn write_report(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.sh.mgr.write(&self.sh.id, bytes);
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

    /// The question watch's rows: the live screen and a screen of history above it.
    pub fn question_rows(&self) -> Vec<String> {
        self.emu.lock().watch_rows()
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

    /// Read only: no dirty flag, no wake.
    pub fn with_emu_quiet<R>(&self, f: impl FnOnce(&Emulator) -> R) -> R {
        f(&self.emu.lock())
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
