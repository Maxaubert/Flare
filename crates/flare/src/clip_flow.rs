//! The copy and paste flows of the terminal (Task 21). Source: PT TerminalPanel.tsx pasteHere,
//! pasteFrom and 'paste-plain', copyNotice.ts, appCopyNotice.ts (M2's AppCopyDeps), rules 1, 5, 9,
//! 10 and 135. Every clipboard read and write runs on a worker thread, except BoundedClip's capped
//! wait (8 ms) for the program-copy watch.
#![allow(dead_code)] // wired by TermUi and the window (Tasks 23 and 24)

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use flare_agent::keys::image_paste_key;
use flare_agent::kinds::AgentKind;
use flare_shells::paste::{ClipboardState, PasteDecision, PathShell, decide_paste, sanitize_paste};
use flare_term::app_copy::AppCopyDeps;
use flare_win::clipboard_port::ClipboardPort;

/// Which paste was asked for: Ctrl+V and the menu's Paste, or Ctrl+Shift+V (plain text only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteMode {
    Normal,
    Plain,
}

/// A paste request, carried through the worker so the answer lands on the right tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteReq {
    pub tab: String,
    pub mode: PasteMode,
}

/// What the paste sends to the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteAction {
    /// The agent's one image key (ESC v for Claude, ^V otherwise).
    ImageKey(&'static str),
    /// Text to send as a paste (bracketed when the program asked for it).
    Text(String),
    /// Send nothing.
    Nothing,
}

/// Reads the clipboard on a worker thread and calls `post` there with the state (the caller posts
/// it to the UI).
pub fn read_async(
    port: Arc<dyn ClipboardPort>,
    req: PasteReq,
    post: impl FnOnce(PasteReq, ClipboardState) + Send + 'static,
) {
    thread::spawn(move || {
        let state = port.read_state();
        post(req, state);
    });
}

/// The decision (pure). Normal: copied files first, as quoted paths for the tab's shell; then an
/// image, as the agent's one key; then sanitised text. `shell` is `path_shell_of(handle.shell_id())`;
/// `None` (the shell has not started) makes a copied-files paste `Nothing` (never guess the quoting)
/// while an image or text still pastes. Plain: only `sanitize_paste(text)`, `Nothing` when empty;
/// files and image are ignored.
pub fn paste_action(
    state: &ClipboardState,
    mode: PasteMode,
    shell: Option<PathShell>,
    agent: Option<AgentKind>,
) -> PasteAction {
    match mode {
        PasteMode::Plain => {
            let text = sanitize_paste(&state.text);
            if text.is_empty() {
                PasteAction::Nothing
            } else {
                PasteAction::Text(text)
            }
        }
        PasteMode::Normal => {
            // Copied files need a shell to quote for. Without one, quoting would be a guess
            // (decide_paste's single quotes are not cmd's), so the paste sends nothing.
            if !state.files.is_empty() && shell.is_none() {
                return PasteAction::Nothing;
            }
            match decide_paste(state, shell) {
                PasteDecision::Key => PasteAction::ImageKey(image_paste_key(agent)),
                PasteDecision::Text(t) => PasteAction::Text(t),
                PasteDecision::None => PasteAction::Nothing,
            }
        }
    }
}

/// A selection of at most this many rows is read on the UI thread, as before; a bigger one is read
/// on a worker in chunks (`TermHandle::selection_text_async`).
pub const SYNC_COPY_ROWS: usize = 200;

/// Whether a selection of `rows` rows is copied inline.
pub fn is_small_copy(rows: usize) -> bool {
    rows <= SYNC_COPY_ROWS
}

/// Writes on a worker thread; `done(true)` only when the clipboard took the exact text. Refuses
/// empty or over `max` (the port does, before it touches the clipboard).
pub fn copy_async(
    port: Arc<dyn ClipboardPort>,
    text: String,
    max: usize,
    done: impl FnOnce(bool) + Send + 'static,
) {
    thread::spawn(move || {
        let ok = port.write_text(&text, max).is_ok();
        done(ok);
    });
}

/// One long-lived clipboard writer for the writes a program can flood (OSC 52). A job that is
/// waiting when a newer one arrives is dropped unanswered, so a flood costs one write and the last
/// text wins, in order, on one thread (audit 2026-10-20, L39 and L48). The thread ends when the
/// copier is dropped.
pub struct LatestCopier {
    tx: mpsc::Sender<CopyJob>,
}

struct CopyJob {
    text: String,
    max: usize,
    done: Box<dyn FnOnce(bool) + Send>,
}

impl LatestCopier {
    pub fn new(port: Arc<dyn ClipboardPort>) -> Self {
        let (tx, rx) = mpsc::channel::<CopyJob>();
        thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let ok = port.write_text(&job.text, job.max).is_ok();
                (job.done)(ok);
            }
        });
        Self { tx }
    }

    /// Queues the write; `done(ok)` runs on the worker unless a newer job replaced this one.
    pub fn submit(&self, text: String, max: usize, done: impl FnOnce(bool) + Send + 'static) {
        let _ = self.tx.send(CopyJob {
            text,
            max,
            done: Box::new(done),
        });
    }
}

/// The program-copy watch's reader. It starts a worker read and waits at most `WAIT_MS`; a read
/// that is late keeps running, and its answer is what the NEXT call returns. The text is compared
/// by the watch and is never kept or logged here.
pub struct BoundedClip {
    port: Arc<dyn ClipboardPort>,
    pending: Option<mpsc::Receiver<Option<String>>>,
}

impl BoundedClip {
    /// The longest the UI thread waits for one read, in milliseconds.
    pub const WAIT_MS: u64 = 8;

    pub fn new(port: Arc<dyn ClipboardPort>) -> Self {
        Self {
            port,
            pending: None,
        }
    }

    /// The clipboard's text, or None when it has none, cannot be read, or is still being read.
    pub fn read(&mut self) -> Option<String> {
        let rx = match self.pending.take() {
            Some(rx) => rx,
            None => {
                let (tx, rx) = mpsc::channel();
                let port = self.port.clone();
                thread::spawn(move || {
                    // The receiver may be gone (a late answer nobody wants); that is fine.
                    let _ = tx.send(port.read_text());
                });
                rx
            }
        };
        let waited = rx.recv_timeout(Duration::from_millis(Self::WAIT_MS)); // sync-ok: the one bounded wait on the UI thread, 8 ms
        match waited {
            Ok(text) => text,
            Err(RecvTimeoutError::Timeout) => {
                self.pending = Some(rx);
                None
            }
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }
}

/// `AppCopyDeps` over a `BoundedClip`, a shared "the program has the mouse" flag and an announce
/// closure (the Copied badge). Used on the UI thread only.
pub struct AppCopyAdapter {
    clip: BoundedClip,
    mouse: Rc<Cell<bool>>,
    announce: Rc<dyn Fn()>,
}

impl AppCopyAdapter {
    pub fn new(
        port: Arc<dyn ClipboardPort>,
        mouse: Rc<Cell<bool>>,
        announce: Rc<dyn Fn()>,
    ) -> Self {
        Self {
            clip: BoundedClip::new(port),
            mouse,
            announce,
        }
    }
}

impl AppCopyDeps for AppCopyAdapter {
    fn read(&mut self) -> Option<String> {
        self.clip.read()
    }

    fn program_has_mouse(&self) -> bool {
        self.mouse.get()
    }

    fn announce(&mut self) {
        (self.announce)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_win::clipboard_port::{CLIPBOARD_WRITE_MAX, MemoryClipboard};
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    fn cs(image: bool, text: &str, files: &[&str]) -> ClipboardState {
        ClipboardState {
            image,
            text: text.to_string(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// A MemoryClipboard that records the thread each call ran on.
    struct Probe {
        inner: MemoryClipboard,
        seen: Mutex<Vec<thread::ThreadId>>,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                inner: MemoryClipboard::new(),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn note(&self) {
            self.seen.lock().unwrap().push(thread::current().id());
        }
    }

    impl ClipboardPort for Probe {
        fn write_text(&self, text: &str, max: usize) -> Result<(), String> {
            self.note();
            self.inner.write_text(text, max)
        }
        fn read_text(&self) -> Option<String> {
            self.note();
            self.inner.read_text()
        }
        fn read_state(&self) -> ClipboardState {
            self.note();
            self.inner.read_state()
        }
        fn sequence(&self) -> u32 {
            self.inner.sequence()
        }
    }

    /// A port whose text read takes `delay`.
    struct Slow {
        delay: Duration,
        text: String,
    }

    impl ClipboardPort for Slow {
        fn write_text(&self, _text: &str, _max: usize) -> Result<(), String> {
            Err("read only".to_string())
        }
        fn read_text(&self) -> Option<String> {
            thread::sleep(self.delay);
            Some(self.text.clone())
        }
        fn read_state(&self) -> ClipboardState {
            ClipboardState::default()
        }
        fn sequence(&self) -> u32 {
            0
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_text_paste_is_the_sanitised_text() {
        assert_eq!(
            paste_action(&cs(false, "ls\n", &[]), PasteMode::Normal, None, None),
            PasteAction::Text("ls\n".to_string())
        );
        assert_eq!(
            paste_action(
                &cs(false, "x\u{1b}[201~rm", &[]),
                PasteMode::Normal,
                None,
                None
            ),
            PasteAction::Text("x[201~rm".to_string())
        );
    }

    #[test]
    fn an_image_beats_text_riding_along() {
        assert_eq!(
            paste_action(&cs(true, "caption", &[]), PasteMode::Normal, None, None),
            PasteAction::ImageKey("\x16")
        );
    }

    #[test]
    fn the_image_key_follows_the_agent() {
        let img = cs(true, "", &[]);
        assert_eq!(
            paste_action(&img, PasteMode::Normal, None, None),
            PasteAction::ImageKey("\x16")
        );
        assert_eq!(
            paste_action(&img, PasteMode::Normal, None, Some(AgentKind::Claude)),
            PasteAction::ImageKey("\x1bv")
        );
        assert_eq!(
            paste_action(&img, PasteMode::Normal, None, Some(AgentKind::Codex)),
            PasteAction::ImageKey("\x16")
        );
    }

    #[test]
    fn copied_files_are_quoted_paths_for_the_tabs_shell() {
        let win = cs(false, "", &["C:\\a b.txt", "C:\\c.txt"]);
        assert_eq!(
            paste_action(&win, PasteMode::Normal, Some(PathShell::PowerShell), None),
            PasteAction::Text("'C:\\a b.txt' 'C:\\c.txt'".to_string())
        );
        let one = cs(false, "", &["C:\\a b.txt"]);
        assert_eq!(
            paste_action(&one, PasteMode::Normal, Some(PathShell::Cmd), None),
            PasteAction::Text("\"C:\\a b.txt\"".to_string())
        );
        let bash = cs(false, "", &["/mnt/c/it's"]);
        assert_eq!(
            paste_action(&bash, PasteMode::Normal, Some(PathShell::Bash), None),
            PasteAction::Text("'/mnt/c/it'\\''s'".to_string())
        );
    }

    #[test]
    fn a_refused_path_pastes_nothing() {
        let esc = cs(false, "", &["C:\\a\u{1b}b.txt"]);
        assert_eq!(
            paste_action(&esc, PasteMode::Normal, Some(PathShell::PowerShell), None),
            PasteAction::Nothing
        );
        let pct = cs(false, "", &["C:\\100%.txt"]);
        assert_eq!(
            paste_action(&pct, PasteMode::Normal, Some(PathShell::Cmd), None),
            PasteAction::Nothing
        );
    }

    #[test]
    fn copied_files_with_no_known_shell_paste_nothing() {
        let files = cs(false, "", &["C:\\a.txt"]);
        assert_eq!(
            paste_action(&files, PasteMode::Normal, None, None),
            PasteAction::Nothing
        );
        let text = cs(false, "hello", &[]);
        assert_eq!(
            paste_action(&text, PasteMode::Normal, None, None),
            PasteAction::Text("hello".to_string())
        );
    }

    #[test]
    fn an_ampersand_path_is_inert_in_cmd_because_cmd_quoting_is_used() {
        let amp = cs(false, "", &["C:\\a&calc.txt"]);
        assert_eq!(
            paste_action(&amp, PasteMode::Normal, Some(PathShell::Cmd), None),
            PasteAction::Text("\"C:\\a&calc.txt\"".to_string())
        );
    }

    #[test]
    fn the_osc52_caps_agree() {
        assert_eq!(
            flare_win::clipboard_port::OSC52_MAX,
            flare_term::osc52::OSC52_MAX
        );
    }

    #[test]
    fn files_beat_an_image_and_text() {
        let all = cs(true, "text", &["C:\\a.txt", "C:\\b.txt"]);
        assert_eq!(
            paste_action(&all, PasteMode::Normal, Some(PathShell::PowerShell), None),
            PasteAction::Text("'C:\\a.txt' 'C:\\b.txt'".to_string())
        );
    }

    #[test]
    fn plain_pastes_only_the_text() {
        let all = cs(true, "plain text", &["C:\\a.txt"]);
        assert_eq!(
            paste_action(
                &all,
                PasteMode::Plain,
                Some(PathShell::PowerShell),
                Some(AgentKind::Claude)
            ),
            PasteAction::Text("plain text".to_string())
        );
    }

    #[test]
    fn plain_of_only_controls_or_empty_is_nothing() {
        assert_eq!(
            paste_action(
                &cs(false, "\u{1b}\u{7f}", &[]),
                PasteMode::Plain,
                None,
                None
            ),
            PasteAction::Nothing
        );
        assert_eq!(
            paste_action(&cs(false, "", &[]), PasteMode::Plain, None, None),
            PasteAction::Nothing
        );
    }

    #[test]
    fn an_empty_clipboard_is_nothing() {
        assert_eq!(
            paste_action(&ClipboardState::default(), PasteMode::Normal, None, None),
            PasteAction::Nothing
        );
    }

    #[test]
    fn read_async_posts_once_with_the_state_off_the_calling_thread() {
        let port = Arc::new(Probe::new());
        port.inner.set_text("from the clipboard");
        let (tx, rx) = channel();
        let req = PasteReq {
            tab: "t7".to_string(),
            mode: PasteMode::Plain,
        };
        read_async(port.clone(), req.clone(), move |r, state| {
            tx.send((thread::current().id(), r, state)).unwrap();
        });
        let (tid, got_req, state) = rx.recv_timeout(WAIT).expect("posted");
        assert_ne!(tid, thread::current().id(), "the read ran on a worker");
        assert_eq!(got_req, req);
        assert_eq!(state.text, "from the clipboard");
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "posted once"
        );
    }

    #[test]
    fn a_small_copy_is_inline_and_immediate() {
        assert!(is_small_copy(0));
        assert!(is_small_copy(SYNC_COPY_ROWS));
        assert!(!is_small_copy(SYNC_COPY_ROWS + 1));
    }
    #[test]
    fn copy_async_writes_the_exact_text_and_reports_success() {
        let port = Arc::new(Probe::new());
        let text = "copied by the terminal".to_string();
        let (tx, rx) = channel();
        copy_async(port.clone(), text.clone(), CLIPBOARD_WRITE_MAX, move |ok| {
            tx.send((thread::current().id(), ok)).unwrap();
        });
        let (tid, ok) = rx.recv_timeout(WAIT).expect("done");
        assert!(ok);
        assert_ne!(tid, thread::current().id(), "the write ran on a worker");
        assert_eq!(port.inner.read_text().as_deref(), Some(text.as_str()));
        let seen = port.seen.lock().unwrap().clone();
        assert!(
            seen.iter().all(|t| *t != thread::current().id()),
            "the port was only ever called from the worker"
        );
    }

    #[test]
    fn copy_async_refuses_empty_and_oversize_without_touching_the_clipboard() {
        let port = Arc::new(MemoryClipboard::new());
        port.set_text("before");
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        copy_async(
            port.clone(),
            String::new(),
            CLIPBOARD_WRITE_MAX,
            move |ok| {
                tx.send(ok).unwrap();
            },
        );
        assert!(!rx.recv_timeout(WAIT).expect("done"));
        copy_async(
            port.clone(),
            "x".repeat(4001),
            CLIPBOARD_WRITE_MAX,
            move |ok| {
                tx2.send(ok).unwrap();
            },
        );
        assert!(!rx.recv_timeout(WAIT).expect("done"));
        assert_eq!(port.read_text().as_deref(), Some("before"));
    }

    #[test]
    fn bounded_clip_returns_a_quick_read() {
        let port = Arc::new(MemoryClipboard::new());
        port.set_text("quick");
        let mut clip = BoundedClip::new(port);
        // A read may miss the 8 ms cap on a loaded machine; the next call returns its answer.
        let got = (0..200).find_map(|_| {
            let r = clip.read();
            if r.is_none() {
                thread::sleep(Duration::from_millis(2));
            }
            r
        });
        assert_eq!(got.as_deref(), Some("quick"));
    }

    #[test]
    fn bounded_clip_gives_up_after_the_cap_and_returns_the_late_answer_next_time() {
        let port = Arc::new(Slow {
            delay: Duration::from_millis(60),
            text: "late".to_string(),
        });
        let mut clip = BoundedClip::new(port);
        let started = Instant::now();
        assert_eq!(clip.read(), None);
        assert!(
            started.elapsed() < Duration::from_millis(40),
            "the UI thread waited {:?}",
            started.elapsed()
        );
        thread::sleep(Duration::from_millis(80));
        assert_eq!(clip.read().as_deref(), Some("late"));
    }

    #[test]
    fn the_copy_watch_announces_when_the_clipboard_changes_after_a_selecting_release() {
        use flare_term::app_copy::AppCopyWatch;

        let port = Arc::new(MemoryClipboard::new());
        let mouse = Rc::new(Cell::new(true));
        let count = Rc::new(Cell::new(0u32));
        let c2 = count.clone();
        let announce: Rc<dyn Fn()> = Rc::new(move || c2.set(c2.get() + 1));
        let mut w = AppCopyWatch::new(AppCopyAdapter::new(port.clone(), mouse, announce));
        w.down(10.0, 10.0, 1, 0);
        w.up(90.0, 10.0, 0);
        port.set_text("new");
        w.advance(200);
        assert_eq!(count.get(), 1, "a changed clipboard announces once");

        // an unchanged clipboard says nothing
        let port = Arc::new(MemoryClipboard::new());
        port.set_text("same");
        let mouse = Rc::new(Cell::new(true));
        let count = Rc::new(Cell::new(0u32));
        let c2 = count.clone();
        let announce: Rc<dyn Fn()> = Rc::new(move || c2.set(c2.get() + 1));
        let mut w = AppCopyWatch::new(AppCopyAdapter::new(port, mouse, announce));
        w.down(10.0, 10.0, 1, 0);
        w.up(90.0, 10.0, 0);
        w.advance(200);
        assert_eq!(count.get(), 0, "an unchanged clipboard announces nothing");
    }
}
