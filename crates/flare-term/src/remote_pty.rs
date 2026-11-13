//! RemotePty: the pty of a tab whose shell runs on another machine (remote sessions PR 3).
//!
//! A remote tab is an ordinary shell tab whose session owns a `RemotePty` instead of a local pty (rule 287). Everything
//! above the pty is the local tab's: the batcher, the writer queue, the resize retry, the emulator, the scrollback and
//! the OSC parsing. Below it the link does the work on its own thread (`flare::remote_link`): it takes what the tab
//! typed and the size it wants out of the [`RemoteIo`], sends them as `remote.input` and `remote.resize`, polls
//! `remote.output` and pushes the bytes in for the pty's reader.
//!
//! Typing never crosses the link as raw bytes (rule 279). The writer turns what the tab wrote back into the keys, text
//! and pastes the person typed ([`decode_typing`]), and the host encodes them again with its own modes. A sequence
//! that is no key (a mouse report, a focus report) is not sent. The answers the tab's emulator gives the program's
//! questions are not typing and never reach the writer: the host's own emulator answers them
//! (`SessionManager::write_reply`).
//!
//! Closing the tab ends the session everywhere (`kill`: `remote.close`); the quit of this Flare and a dropped pty only
//! let go of it (`leave`: `remote.detach`), and the host session goes on (rule 288).

use crate::handle::RemoteIntent;
use crate::keys::KeyEvent;
use crate::pty::PtyProcess;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The most output bytes the pty holds for its reader; past it the link waits before it pushes more.
pub const FEED_MAX: usize = 4 << 20;

/// Where a remote tab's link stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Starting ssh, saying hello, opening or attaching.
    Connecting,
    /// Attached: output is polled and typing goes over.
    Live,
    /// The link broke; the next try is on its way (back-off 1 s to 30 s).
    Down,
    /// The tab's session ended or the tab let go of it.
    Ended,
}

impl LinkState {
    fn code(self) -> u8 {
        match self {
            LinkState::Connecting => 0,
            LinkState::Live => 1,
            LinkState::Down => 2,
            LinkState::Ended => 3,
        }
    }

    fn of(code: u8) -> Self {
        match code {
            1 => LinkState::Live,
            2 => LinkState::Down,
            3 => LinkState::Ended,
            _ => LinkState::Connecting,
        }
    }
}

#[derive(Default)]
struct LiveInner {
    host_tab: Mutex<Option<String>>,
    seq: AtomicU64,
    state: AtomicU8,
    /// The host's newest status of the tab that the window has not taken yet (remote sessions PR 4).
    status: Mutex<Option<HostStatus>>,
    /// File work for the link, in the order asked (remote sessions PR 6).
    errands: Mutex<VecDeque<Errand>>,
}

/// A file of the host the link fetched for a click on a file link (remote sessions PR 6, rule 303): the host's name
/// for it (its last part) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub name: String,
    pub data: Vec<u8>,
}

/// A file of this machine to copy to the host (a dropped file, a pasted image; rule 304): the name to keep it under
/// there (the host cleans it again) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub name: String,
    pub data: Vec<u8>,
}

/// Why an errand did not get done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrandFail {
    /// The host has no file by any spelling the link was given, or none it may send (rule 303 says nothing more).
    NoSuchFile,
    /// The link is down, the host refused, or the file is too big: a line for the person.
    Failed(String),
}

type FetchDone = Box<dyn FnOnce(Result<Fetched, ErrandFail>) + Send>;
type PutDone = Box<dyn FnOnce(Result<Vec<String>, ErrandFail>) + Send>;

/// File work a remote tab's link does between its polls, on its own worker (rule 34): the window asks, the link answers
/// through `done` (on the link's thread: `done` posts to the window).
pub enum Errand {
    /// Fetch a host file a file link names: each spelling in turn (`texts`), read by the host from its own folder of
    /// the tab (rule 303).
    Fetch { texts: Vec<String>, done: FetchDone },
    /// Copy files to the host's upload folder of the tab, then paste their host paths into the tab as one paste, quoted
    /// by the host for its shell (rule 304). `done` gets the host paths.
    Put { files: Vec<Upload>, done: PutDone },
}

impl Errand {
    /// Tells the asker the errand will not be done.
    pub fn fail(self, why: ErrandFail) {
        match self {
            Errand::Fetch { done, .. } => done(Err(why)),
            Errand::Put { done, .. } => done(Err(why)),
        }
    }
}

/// What the host says of a remote tab (`remote.status`, remote sessions PR 4, rule 294): words its own marks give, never
/// a mark to copy. The viewer's hub turns them into its own marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostStatus {
    /// `working`, `question`, `done`, `failed` or `idle` (`flare_ctl::remote::TabState`).
    pub state: String,
    /// The agent the host found in the tab (`claude`, `codex`, `agy`), if any.
    pub agent: Option<String>,
    /// The tab's Claude session (a lower-case UUID), if the host knows one.
    pub session: Option<String>,
    /// The words of the question the host's screen shows, while it asks.
    pub question: Option<String>,
    /// The kind of the failure, while it failed.
    pub failed: Option<String>,
}

/// What a remote tab has reached, shared by its link (which writes it) and the window (which saves it in `tabs.json`):
/// the host's id of the tab and the number of the host's output read so far. Two values are equal only when they are
/// the same tab's.
#[derive(Clone, Default)]
pub struct RemoteLive(Arc<LiveInner>);

impl RemoteLive {
    /// A tab that is opened fresh (`host_tab` None) or brought back (`host_tab` Some, from `tabs.json`).
    pub fn new(host_tab: Option<String>, seq: u64) -> Self {
        let live = Self::default();
        *lk(&live.0.host_tab) = host_tab;
        live.0.seq.store(seq, Ordering::SeqCst);
        live
    }

    /// The host's id of the tab, once `remote.open` answered (or as restored).
    pub fn host_tab(&self) -> Option<String> {
        lk(&self.0.host_tab).clone()
    }

    pub fn set_host_tab(&self, tab: &str) {
        *lk(&self.0.host_tab) = Some(tab.to_string());
    }

    /// The host's output number the tab has read up to.
    pub fn seq(&self) -> u64 {
        self.0.seq.load(Ordering::SeqCst)
    }

    pub fn set_seq(&self, seq: u64) {
        self.0.seq.store(seq, Ordering::SeqCst);
    }

    pub fn link(&self) -> LinkState {
        LinkState::of(self.0.state.load(Ordering::SeqCst))
    }

    pub fn set_link(&self, s: LinkState) {
        self.0.state.store(s.code(), Ordering::SeqCst);
    }

    /// The link heard a new status of the tab from the host (it keeps only the newest until the window takes it).
    pub fn set_status(&self, s: HostStatus) {
        *lk(&self.0.status) = Some(s);
    }

    /// The host's status the window has not taken yet, taken (the UI thread, at each wake).
    pub fn take_status(&self) -> Option<HostStatus> {
        lk(&self.0.status).take()
    }

    /// File work for the link (the window): done between the link's polls, in the order asked. A tab whose link has
    /// ended is told at once.
    pub fn send_errand(&self, e: Errand) {
        let mut q = lk(&self.0.errands);
        if self.link() == LinkState::Ended {
            drop(q);
            e.fail(ErrandFail::Failed("this tab's session has ended".into()));
            return;
        }
        q.push_back(e);
    }

    /// The errands asked since the last take (the link's worker).
    pub fn take_errands(&self) -> Vec<Errand> {
        lk(&self.0.errands).drain(..).collect()
    }

    /// The link ends: no errand is taken any more, and the ones still waiting are handed back to be failed. Under the
    /// errands' lock, so an errand is either handed back here or told at once by [`RemoteLive::send_errand`].
    pub fn end_errands(&self) -> Vec<Errand> {
        let mut q = lk(&self.0.errands);
        self.set_link(LinkState::Ended);
        q.drain(..).collect()
    }
}

impl PartialEq for RemoteLive {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RemoteLive {}

impl std::fmt::Debug for RemoteLive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLive")
            .field("host_tab", &self.host_tab())
            .field("seq", &self.seq())
            .field("link", &self.link())
            .finish()
    }
}

/// Which session on which machine a remote tab shows (`SpawnSpec::remote`). No token and no environment: the host
/// builds its shell's own (rule 290).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSpec {
    /// The machine's id in `machines.json`.
    pub machine: String,
    /// The folder on the host the session works in: a host path, never statted or opened here.
    pub folder: String,
    /// The Claude session to resume in the host's new tab (remote sessions PR 5): sent with `remote.open` only, so a
    /// tab that already has its host tab never starts it again.
    pub resume: Option<String>,
    /// The tab's live state (the host's id of the tab, the output reached).
    pub live: RemoteLive,
}

/// How a remote tab lets go of its host session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaving {
    /// The tab was closed: the session ends everywhere (`remote.close`, which asks as any close does, rule 48).
    Close,
    /// This Flare quits or the pty was dropped: only this viewer goes (`remote.detach`); the session goes on.
    Detach,
}

/// What the link has to send now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub intents: Vec<RemoteIntent>,
    /// The last size the tab asked for since the last take (resizes are coalesced).
    pub size: Option<(u16, u16)>,
    pub leaving: Option<Leaving>,
}

#[derive(Default)]
struct Outbox {
    intents: Vec<RemoteIntent>,
    size: Option<(u16, u16)>,
    leaving: Option<Leaving>,
}

#[derive(Default)]
struct Feed {
    chunks: VecDeque<Vec<u8>>,
    held: usize,
    closed: bool,
}

type ExitCb = Box<dyn FnOnce(Option<u32>) + Send>;

#[derive(Default)]
struct ExitState {
    done: bool,
    code: Option<u32>,
    cb: Option<ExitCb>,
}

/// The meeting point of a remote tab's pty (the session's side) and its link (the worker's side).
#[derive(Default)]
pub struct RemoteIo {
    out: Mutex<Outbox>,
    out_cv: Condvar,
    feed: Mutex<Feed>,
    feed_cv: Condvar,
    exit: Mutex<ExitState>,
}

impl RemoteIo {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    // ---- the link's side ----

    /// Waits up to `wait` for typing, a size or the end, and takes what there is (maybe nothing).
    pub fn take_outgoing(&self, wait: Duration) -> Outgoing {
        let mut o = lk(&self.out);
        if o.intents.is_empty() && o.size.is_none() && o.leaving.is_none() && !wait.is_zero() {
            o = self
                .out_cv
                .wait_timeout(o, wait) // sync-ok: the link's worker thread waits for typing, never the UI thread
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        Outgoing {
            intents: std::mem::take(&mut o.intents),
            size: o.size.take(),
            leaving: o.leaving,
        }
    }

    /// The tab let go (closed or left), without taking anything.
    pub fn leaving(&self) -> Option<Leaving> {
        lk(&self.out).leaving
    }

    /// Output from the host, for the pty's reader. Waits while the reader is far behind (the session's own throttle
    /// reached), and drops the bytes once the tab is gone.
    pub fn push_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut f = lk(&self.feed);
        while f.held >= FEED_MAX && !f.closed {
            f = self
                .feed_cv
                .wait_timeout(f, Duration::from_millis(200)) // sync-ok: the link's worker thread waits for the reader
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        if f.closed {
            return;
        }
        f.held += bytes.len();
        f.chunks.push_back(bytes.to_vec());
        drop(f);
        self.feed_cv.notify_all();
    }

    /// The session ended (on the host, or here): the reader reads the end and the exit is told once.
    pub fn finish(&self, code: Option<u32>) {
        {
            let mut f = lk(&self.feed);
            f.closed = true;
        }
        self.feed_cv.notify_all();
        let cb = {
            let mut e = lk(&self.exit);
            if e.done {
                return;
            }
            e.done = true;
            e.code = code;
            e.cb.take()
        };
        if let Some(cb) = cb {
            cb(code);
        }
    }

    // ---- the pty's side ----

    fn type_in(&self, intents: Vec<RemoteIntent>) {
        if intents.is_empty() {
            return;
        }
        let mut o = lk(&self.out);
        if o.leaving.is_some() {
            return;
        }
        o.intents.extend(intents);
        drop(o);
        self.out_cv.notify_all();
    }

    fn want_size(&self, cols: u16, rows: u16) {
        let mut o = lk(&self.out);
        if o.leaving.is_some() {
            return;
        }
        o.size = Some((cols, rows));
        drop(o);
        self.out_cv.notify_all();
    }

    /// The tab lets go: the first way asked wins. The pty's end is at once (a dead link never holds a close or the quit,
    /// rule 28): the reader reads the end and the exit is told; the link sends the close or the detach on its own time.
    fn leave(&self, how: Leaving) {
        {
            let mut o = lk(&self.out);
            if o.leaving.is_none() {
                o.leaving = Some(how);
            }
        }
        self.out_cv.notify_all();
        self.finish(Some(0));
    }

    fn read_into(&self, buf: &mut [u8]) -> usize {
        let mut f = lk(&self.feed);
        loop {
            if let Some(front) = f.chunks.front_mut() {
                let n = front.len().min(buf.len());
                buf[..n].copy_from_slice(&front[..n]);
                front.drain(..n);
                if front.is_empty() {
                    f.chunks.pop_front();
                }
                f.held -= n;
                drop(f);
                self.feed_cv.notify_all();
                return n;
            }
            if f.closed {
                return 0;
            }
            f = self.feed_cv.wait(f).unwrap_or_else(|p| p.into_inner()); // sync-ok: the session's reader thread
        }
    }

    /// The bytes the pty holds for its reader (tests).
    pub fn held(&self) -> usize {
        lk(&self.feed).held
    }
}

struct FeedReader(Arc<RemoteIo>);

impl Read for FeedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        Ok(self.0.read_into(buf))
    }
}

struct TypingWriter(Arc<RemoteIo>);

impl Write for TypingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.type_in(decode_typing(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The pty of a remote tab. Its process id is 0, and it is never in the list of live process ids: no process of this
/// machine is the tab's.
pub struct RemotePty {
    io: Arc<RemoteIo>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
}

impl RemotePty {
    pub fn new(io: Arc<RemoteIo>) -> Self {
        Self {
            reader: Some(Box::new(FeedReader(Arc::clone(&io)))),
            writer: Some(Box::new(TypingWriter(Arc::clone(&io)))),
            io,
        }
    }
}

impl PtyProcess for RemotePty {
    fn pid(&self) -> u32 {
        0
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        self.io.want_size(cols, rows);
        Ok(())
    }

    /// Closing the tab: the session ends everywhere (`remote.close`). Never blocks.
    fn kill(&mut self) {
        self.io.leave(Leaving::Close);
    }

    /// The quit of this Flare: only this viewer goes (`remote.detach`); the host session goes on.
    fn leave(&mut self) {
        self.io.leave(Leaving::Detach);
    }

    fn exit_code(&mut self) -> Option<u32> {
        lk(&self.io.exit).code
    }

    fn on_exit(&mut self, cb: Box<dyn FnOnce(Option<u32>) + Send>) {
        let mut st = lk(&self.io.exit);
        if st.done {
            let code = st.code;
            drop(st);
            cb(code);
        } else {
            st.cb = Some(cb);
        }
    }
}

impl Drop for RemotePty {
    /// A pty dropped without a kill (its session ended on the host) only lets go: nothing here ever closes a host
    /// session by accident.
    fn drop(&mut self) {
        self.io.leave(Leaving::Detach);
    }
}

// ---- typing back into intents ----

fn ctrl_letter(b: u8) -> KeyEvent {
    let c = char::from(b'a' + b - 1);
    KeyEvent::new(&c.to_string(), &format!("Key{}", c.to_ascii_uppercase())).ctrl()
}

/// The key a control byte is: Enter, Tab, Backspace and the Ctrl chords `encode_key` makes.
fn control_key(b: u8) -> Option<KeyEvent> {
    Some(match b {
        0x0d => KeyEvent::new("Enter", "Enter"),
        0x0a => KeyEvent::new("Enter", "Enter").ctrl(),
        0x09 => KeyEvent::new("Tab", "Tab"),
        0x7f => KeyEvent::new("Backspace", "Backspace"),
        0x08 => KeyEvent::new("Backspace", "Backspace").ctrl(),
        _ => return ctrl_chord(b),
    })
}

/// The Ctrl chord a control byte is, by letter or punctuation (never Enter or Tab by name).
fn ctrl_chord(b: u8) -> Option<KeyEvent> {
    Some(match b {
        0x00 => KeyEvent::new(" ", "Space").ctrl(),
        0x01..=0x1a => ctrl_letter(b),
        0x1c => KeyEvent::new("\\", "Backslash").ctrl(),
        0x1d => KeyEvent::new("]", "BracketRight").ctrl(),
        0x1e => KeyEvent::new("6", "Digit6").ctrl(),
        0x1f => KeyEvent::new("/", "Slash").ctrl(),
        _ => return None,
    })
}

/// xterm's modifier parameter (1 + shift + 2 alt + 4 ctrl + 8 meta) on a key.
fn with_mods(mut k: KeyEvent, m: u32) -> KeyEvent {
    let bits = m.saturating_sub(1);
    k.shift = bits & 1 != 0;
    k.alt = bits & 2 != 0;
    k.ctrl = bits & 4 != 0;
    k.meta = bits & 8 != 0;
    k
}

fn named(name: &str) -> KeyEvent {
    KeyEvent::new(name, name)
}

/// The key a CSI sequence is (`params` between `ESC [` and the final byte), or None for anything that is no key.
fn csi_key(params: &str, fin: u8) -> Option<KeyEvent> {
    let nums: Vec<u32> = if params.is_empty() {
        Vec::new()
    } else {
        params
            .split(';')
            .map(|p| p.parse::<u32>().ok())
            .collect::<Option<Vec<u32>>>()?
    };
    let mods = match nums.as_slice() {
        [] | [_] => 1,
        [_, m] => *m,
        _ => return None,
    };
    let letter = |name: &str| match nums.as_slice() {
        [] => Some(named(name)),
        [1, m] => Some(with_mods(named(name), *m)),
        _ => None,
    };
    match fin {
        b'A' => letter("ArrowUp"),
        b'B' => letter("ArrowDown"),
        b'C' => letter("ArrowRight"),
        b'D' => letter("ArrowLeft"),
        b'H' => letter("Home"),
        b'F' => letter("End"),
        b'P' | b'Q' | b'R' | b'S' if nums.len() == 2 && nums[0] == 1 => {
            let n = ["F1", "F2", "F3", "F4"][usize::from(fin - b'P')];
            Some(with_mods(named(n), mods))
        }
        b'Z' if nums.is_empty() => Some(named("Tab").shift()),
        b'~' => {
            let name = match nums.first()? {
                2 => "Insert",
                3 => "Delete",
                5 => "PageUp",
                6 => "PageDown",
                15 => "F5",
                17 => "F6",
                18 => "F7",
                19 => "F8",
                20 => "F9",
                21 => "F10",
                23 => "F11",
                24 => "F12",
                _ => return None,
            };
            Some(with_mods(named(name), mods))
        }
        _ => None,
    }
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Skips a string sequence (OSC, DCS, APC, PM) from just after its introducer: to BEL or ST, or the end.
fn skip_string(s: &[u8], from: usize) -> usize {
    let mut j = from;
    while j < s.len() {
        if s[j] == 0x07 {
            return j + 1;
        }
        if s[j] == 0x1b && s.get(j + 1) == Some(&b'\\') {
            return j + 2;
        }
        j += 1;
    }
    s.len()
}

/// One escape sequence at the start of `s` (`s[0]` is ESC): the intent it is, if any, and the bytes it used.
fn decode_escape(s: &[u8]) -> (Option<RemoteIntent>, usize) {
    let Some(&next) = s.get(1) else {
        return (Some(RemoteIntent::Key(named("Escape"))), 1);
    };
    match next {
        b'[' => {
            if s.starts_with(PASTE_START) {
                let body = &s[PASTE_START.len()..];
                let (text, used) = match find(body, PASTE_END) {
                    Some(end) => (&body[..end], PASTE_START.len() + end + PASTE_END.len()),
                    None => (body, s.len()),
                };
                let text = String::from_utf8_lossy(text).into_owned();
                return (
                    (!text.is_empty()).then_some(RemoteIntent::Paste(text)),
                    used,
                );
            }
            let mut j = 2;
            while j < s.len() && (0x20..=0x3f).contains(&s[j]) {
                j += 1;
            }
            match s.get(j) {
                Some(&fin) if (0x40..=0x7e).contains(&fin) => {
                    let params = String::from_utf8_lossy(&s[2..j]);
                    (csi_key(&params, fin).map(RemoteIntent::Key), j + 1)
                }
                // `ESC [` with nothing after it is Alt+[ (what `encode_key` makes of it).
                None if j == 2 => (Some(RemoteIntent::Key(alt_char('['))), 2),
                _ => (None, j.min(s.len())),
            }
        }
        b'O' => match s.get(2) {
            Some(&f) => {
                let name = match f {
                    b'A' => "ArrowUp",
                    b'B' => "ArrowDown",
                    b'C' => "ArrowRight",
                    b'D' => "ArrowLeft",
                    b'H' => "Home",
                    b'F' => "End",
                    b'P' => "F1",
                    b'Q' => "F2",
                    b'R' => "F3",
                    b'S' => "F4",
                    _ => return (None, 3),
                };
                (Some(RemoteIntent::Key(named(name))), 3)
            }
            None => (Some(RemoteIntent::Key(alt_char('O'))), 2),
        },
        b']' | b'P' | b'_' | b'^' => (None, skip_string(s, 2)),
        0x1b => (Some(RemoteIntent::Key(named("Escape").alt())), 2),
        0x0d => (Some(RemoteIntent::Key(named("Enter").alt())), 2),
        0x7f => (Some(RemoteIntent::Key(named("Backspace").alt())), 2),
        b if b < 0x20 => (ctrl_chord(b).map(|k| RemoteIntent::Key(k.alt())), 2),
        _ => {
            // Alt with a character: as many bytes as the character has.
            let len = match next {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            let end = (1 + len).min(s.len());
            match std::str::from_utf8(&s[1..end])
                .ok()
                .and_then(|t| t.chars().next())
            {
                Some(c) if !c.is_control() => (Some(RemoteIntent::Key(alt_char(c))), end),
                _ => (None, end),
            }
        }
    }
}

fn alt_char(c: char) -> KeyEvent {
    let t = c.to_string();
    KeyEvent::new(&t, "").alt().text(&t)
}

fn flush_text(text: &mut Vec<u8>, out: &mut Vec<RemoteIntent>) {
    if text.is_empty() {
        return;
    }
    let clean: String = String::from_utf8_lossy(text)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    text.clear();
    if clean.is_empty() {
        return;
    }
    if let Some(RemoteIntent::Text(t)) = out.last_mut() {
        t.push_str(&clean);
    } else {
        out.push(RemoteIntent::Text(clean));
    }
}

/// Pure: what a tab wrote to its pty, as the keys, text and pastes it was (rule 279). `encode_key` read backwards: a
/// control byte is its Ctrl chord (or Enter, Tab, Backspace), an escape sequence the named key with its modifiers
/// (CSI and SS3 alike, so the host encodes the arrows with its own cursor mode), `ESC` and a character that character
/// with Alt, a bracketed paste a paste, and every run of plain characters a text. Anything else (a mouse or focus
/// report, a string sequence) is no typing and is left out. Each write is read on its own: a key never spans two.
pub fn decode_typing(bytes: &[u8]) -> Vec<RemoteIntent> {
    let mut out = Vec::new();
    let mut text = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            flush_text(&mut text, &mut out);
            let (intent, used) = decode_escape(&bytes[i..]);
            out.extend(intent);
            i += used.max(1);
        } else if b < 0x20 || b == 0x7f {
            flush_text(&mut text, &mut out);
            out.extend(control_key(b).map(RemoteIntent::Key));
            i += 1;
        } else {
            text.push(b);
            i += 1;
        }
    }
    flush_text(&mut text, &mut out);
    out
}

// ---- replayed output ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strip {
    Plain,
    /// An ESC at the end of the last chunk.
    Esc {
        replay: bool,
    },
    /// `ESC ]` and the digits of the ident so far.
    Ident {
        replay: bool,
    },
    /// Inside a signal that is being taken out; `esc` after an ESC (an ST may follow).
    Drop {
        esc: bool,
    },
}

/// Takes the agent's signals (OSC 777 and OSC 7501) out of output that is replayed: the output a reconnect catches up
/// on. A replay is the past, and the past never raises a Finished, a Question or a working mark on this machine
/// (rule 289); the host's own state crosses the link as words instead (`remote.status`, PR 4). A signal belongs to the
/// output it began in, so one cut across the end of the replay is still taken out whole. Everything else passes.
#[derive(Debug)]
pub struct SignalStrip {
    st: Strip,
    held: Vec<u8>,
}

impl Default for SignalStrip {
    fn default() -> Self {
        Self {
            st: Strip::Plain,
            held: Vec::new(),
        }
    }
}

impl SignalStrip {
    pub fn new() -> Self {
        Self::default()
    }

    /// The chunk with the signals that began in replayed output taken out.
    pub fn feed(&mut self, bytes: &[u8], replay: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            self.st = match self.st {
                Strip::Plain => {
                    if b == 0x1b {
                        Strip::Esc { replay }
                    } else {
                        out.push(b);
                        Strip::Plain
                    }
                }
                Strip::Esc { replay: r } => {
                    if b == b']' {
                        self.held = vec![0x1b, b']'];
                        Strip::Ident { replay: r }
                    } else {
                        out.push(0x1b);
                        if b == 0x1b {
                            Strip::Esc { replay }
                        } else {
                            out.push(b);
                            Strip::Plain
                        }
                    }
                }
                Strip::Ident { replay: r } => {
                    if b.is_ascii_digit() && self.held.len() < 8 {
                        self.held.push(b);
                        Strip::Ident { replay: r }
                    } else {
                        let ident = &self.held[2..];
                        if r && b == b';' && (ident == b"777" || ident == b"7501") {
                            self.held.clear();
                            Strip::Drop { esc: false }
                        } else {
                            out.append(&mut self.held);
                            if b == 0x1b {
                                Strip::Esc { replay }
                            } else {
                                out.push(b);
                                Strip::Plain
                            }
                        }
                    }
                }
                Strip::Drop { esc } => {
                    if b == 0x07 || (esc && b == b'\\') {
                        Strip::Plain
                    } else {
                        Strip::Drop { esc: b == 0x1b }
                    }
                }
            };
        }
        out
    }
}

/// Pure: the end of a snapshot that puts the cursor back where the host's is (rule 293). The snapshot's last line is
/// the host screen's bottom row, so the cursor goes to the start of that line, up to the cursor's row and right to its
/// column. Moves relative to the bottom hold on a viewer of another height too.
pub fn snapshot_cursor(rows: usize, line: i32, col: usize) -> Vec<u8> {
    let line = usize::try_from(line)
        .unwrap_or(0)
        .min(rows.saturating_sub(1));
    let up = rows.saturating_sub(1) - line;
    let mut out = b"\r".to_vec();
    if up > 0 {
        out.extend_from_slice(format!("\x1b[{up}A").as_bytes());
    }
    if col > 0 {
        out.extend_from_slice(format!("\x1b[{col}C").as_bytes());
    }
    out
}

/// The length of a cursor move up or right (`ESC [ n A`, `ESC [ n C`) at the start of `s`, if one is there.
fn cursor_move(s: &[char]) -> Option<usize> {
    if s.get(1) != Some(&'[') {
        return None;
    }
    let digits = s[2..].iter().take_while(|c| c.is_ascii_digit()).count();
    let fin = *s.get(2 + digits)?;
    (digits > 0 && digits <= 5 && (fin == 'A' || fin == 'C')).then_some(3 + digits)
}

/// A remote snapshot is text: whatever escape sequence it holds beyond its own clear and home and the cursor moves up
/// and right that put the cursor back (rule 293) is taken out, so a snapshot can never raise a mark or set a title on
/// this machine (rule 289). Line breaks stay.
pub fn plain_snapshot(data: &[u8]) -> Vec<u8> {
    const HEAD: &[u8] = b"\x1b[H\x1b[2J\x1b[3J";
    let body = data.strip_prefix(HEAD).unwrap_or(data);
    let mut out = HEAD.to_vec();
    let chars: Vec<char> = String::from_utf8_lossy(body).chars().collect();
    let mut kept = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(n) = (c == '\x1b').then(|| cursor_move(&chars[i..])).flatten() {
            kept.extend(&chars[i..i + n]);
            i += n;
            continue;
        }
        if c == '\r' || c == '\n' || !c.is_control() {
            kept.push(c);
        }
        i += 1;
    }
    out.extend_from_slice(kept.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_encode::encode_key;
    use std::time::Instant;

    fn key(k: &str, code: &str) -> KeyEvent {
        KeyEvent::new(k, code)
    }

    /// Every key `encode_key` makes into bytes comes back from `decode_typing` as a key that the host encodes into the
    /// same bytes, in both cursor modes: what the host types is what this machine would have typed.
    #[test]
    fn decoded_typing_encodes_to_the_same_bytes_on_the_host() {
        let mut keys: Vec<KeyEvent> = Vec::new();
        for c in 'a'..='z' {
            let code = format!("Key{}", c.to_ascii_uppercase());
            let t = c.to_string();
            keys.push(key(&t, &code).ctrl());
            keys.push(key(&t, &code).alt().text(&t));
            keys.push(key(&t, &code).ctrl().alt());
        }
        for name in [
            "Enter",
            "Tab",
            "Backspace",
            "Escape",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Home",
            "End",
            "Insert",
            "Delete",
            "PageUp",
            "PageDown",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
        ] {
            for m in 0..16u8 {
                let mut k = key(name, name);
                k.shift = m & 1 != 0;
                k.alt = m & 2 != 0;
                k.ctrl = m & 4 != 0;
                k.meta = m & 8 != 0;
                keys.push(k);
            }
        }
        for (k, code) in [
            (" ", "Space"),
            ("\\", "Backslash"),
            ("]", "BracketRight"),
            ("6", "Digit6"),
            ("/", "Slash"),
        ] {
            keys.push(key(k, code).ctrl());
        }
        for app_cursor in [false, true] {
            for k in &keys {
                let Some(bytes) = encode_key(k, app_cursor) else {
                    continue;
                };
                let back = decode_typing(&bytes);
                let mut again = Vec::new();
                for i in &back {
                    match i {
                        RemoteIntent::Key(e) => {
                            again.extend(encode_key(e, app_cursor).unwrap_or_default());
                        }
                        RemoteIntent::Text(t) => again.extend(t.as_bytes()),
                        RemoteIntent::Paste(_) => panic!("a key is no paste: {k:?}"),
                    }
                }
                assert_eq!(again, bytes, "{k:?} (app cursor {app_cursor}) -> {back:?}");
            }
        }
    }

    #[test]
    fn plain_characters_are_one_text_and_a_bracketed_paste_is_a_paste() {
        assert_eq!(
            decode_typing("echo héllo".as_bytes()),
            vec![RemoteIntent::Text("echo héllo".into())]
        );
        assert_eq!(
            decode_typing(b"ls\r"),
            vec![
                RemoteIntent::Text("ls".into()),
                RemoteIntent::Key(key("Enter", "Enter")),
            ]
        );
        assert_eq!(
            decode_typing(b"\x1b[200~one\r\ntwo\x1b[201~"),
            vec![RemoteIntent::Paste("one\r\ntwo".into())]
        );
    }

    /// Rule 279: a report the tab makes is no typing, and no raw escape sequence ever becomes text.
    #[test]
    fn mouse_focus_and_string_sequences_are_never_sent() {
        for b in [
            &b"\x1b[<0;10;5M"[..],
            b"\x1b[<64;3;3m",
            b"\x1b[I",
            b"\x1b[O",
            b"\x1b]11;rgb:0000/0000/0000\x07",
            b"\x1bP1$r0m\x1b\\",
            b"\x1b[?1;2c",
            b"\x1b[99;99;99x",
        ] {
            assert!(decode_typing(b).is_empty(), "{b:?}");
        }
        for i in decode_typing(b"a\x1b[<0;1;1Mb\x9b2J") {
            if let RemoteIntent::Text(t) = i {
                assert!(!t.chars().any(char::is_control), "{t:?}");
            }
        }
    }

    fn pty() -> (RemotePty, Arc<RemoteIo>) {
        let io = RemoteIo::new();
        (RemotePty::new(Arc::clone(&io)), io)
    }

    /// The writer batches typing into intents, the resize keeps only the last size, and the link takes both at once.
    #[test]
    fn typing_and_sizes_wait_for_the_link_and_sizes_coalesce() {
        let (mut p, io) = pty();
        let mut w = p.take_writer().unwrap();
        w.write_all(b"ab").unwrap();
        w.write_all(b"\r").unwrap();
        p.resize(80, 24).unwrap();
        p.resize(100, 30).unwrap();
        let o = io.take_outgoing(Duration::ZERO);
        assert_eq!(
            o.intents,
            vec![
                RemoteIntent::Text("ab".into()),
                RemoteIntent::Key(key("Enter", "Enter")),
            ]
        );
        assert_eq!(o.size, Some((100, 30)));
        assert_eq!(o.leaving, None);
        assert_eq!(io.take_outgoing(Duration::ZERO), Outgoing::default());
    }

    /// The link's output reaches the reader in order, and the end is an end-of-file.
    #[test]
    fn pushed_output_is_read_in_order_and_the_finish_is_the_end() {
        let (mut p, io) = pty();
        let mut r = p.take_reader().unwrap();
        io.push_output(b"one ");
        io.push_output(b"two");
        let mut buf = [0u8; 64];
        let mut got = Vec::new();
        while got.len() < 7 {
            let n = r.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"one two");
        let (tx, rx) = std::sync::mpsc::channel();
        p.on_exit(Box::new(move |c| {
            let _ = tx.send(c);
        }));
        io.finish(Some(3));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Some(3));
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        assert_eq!(p.exit_code(), Some(3));
    }

    /// Rule 28: a kill never waits for the link. The pty ends at once, the link is told to close the session, and
    /// nothing typed after it goes anywhere.
    #[test]
    fn a_kill_ends_at_once_and_asks_the_link_to_close() {
        let (mut p, io) = pty();
        let mut r = p.take_reader().unwrap();
        let mut w = p.take_writer().unwrap();
        let t = Instant::now();
        p.kill();
        assert!(t.elapsed() < Duration::from_millis(100));
        assert!(p.exit_code().is_some());
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        w.write_all(b"late").unwrap();
        let o = io.take_outgoing(Duration::ZERO);
        assert_eq!(o.leaving, Some(Leaving::Close));
        assert!(o.intents.is_empty());
    }

    /// Rule 288: the quit and a dropped pty only detach; a detach after a close is still a close.
    #[test]
    fn a_leave_or_a_drop_detaches_and_never_closes() {
        let (mut p, io) = pty();
        p.leave();
        assert_eq!(io.leaving(), Some(Leaving::Detach));
        let (p2, io2) = pty();
        drop(p2);
        assert_eq!(io2.leaving(), Some(Leaving::Detach));
        let (mut p3, io3) = pty();
        p3.kill();
        drop(p3);
        assert_eq!(io3.leaving(), Some(Leaving::Close));
        assert_eq!(p.pid(), 0);
    }

    /// Rule 289: the agent's signals in replayed output are taken out, even cut across chunks and the end of the
    /// replay; live output keeps them, and every other sequence passes untouched.
    #[test]
    fn signals_are_taken_out_of_replayed_output_only() {
        let mut s = SignalStrip::new();
        let replay = s.feed(
            b"a\x1b]777;flare-agent;state=done\x07b\x1b]7501;state=idle\x1b\\c\x1b]0;title\x07d",
            true,
        );
        assert_eq!(replay, b"abc\x1b]0;title\x07d");
        let live = s.feed(b"\x1b]777;flare-agent;state=done\x07", false);
        assert_eq!(live, b"\x1b]777;flare-agent;state=done\x07");
        // Cut in the ident, in the body, and across the end of the replay.
        let mut s = SignalStrip::new();
        let mut out = s.feed(b"x\x1b]77", true);
        out.extend(s.feed(b"7;state=wor", true));
        out.extend(s.feed(b"king\x07y\x1b", false));
        out.extend(s.feed(b"[31mz", false));
        assert_eq!(out, b"xy\x1b[31mz");
        let mut s = SignalStrip::new();
        assert_eq!(s.feed(b"\x1b]7777;x\x07", true), b"\x1b]7777;x\x07");
    }

    #[test]
    fn a_snapshot_is_text_with_no_sequence_of_its_own() {
        let snap = plain_snapshot(b"\x1b[H\x1b[2J\x1b[3Jone\r\ntwo\x1b]777;state=done\x07");
        assert_eq!(snap, b"\x1b[H\x1b[2J\x1b[3Jone\r\ntwo]777;state=done");
    }

    /// Rule 293: the moves up and right that put the cursor back stay; any other sequence still loses its ESC.
    #[test]
    fn a_snapshot_keeps_only_the_cursor_moves_up_and_right() {
        let end = snapshot_cursor(24, 1, 15);
        assert_eq!(end, b"\r\x1b[22A\x1b[15C");
        assert_eq!(snapshot_cursor(24, 23, 0), b"\r");
        let mut data = b"\x1b[H\x1b[2J\x1b[3Jone\x1b[5B\x1b[2;3H\x1b[A".to_vec();
        data.extend_from_slice(&end);
        let mut want = b"\x1b[H\x1b[2J\x1b[3Jone[5B[2;3H[A".to_vec();
        want.extend_from_slice(&end);
        assert_eq!(plain_snapshot(&data), want);
    }

    #[test]
    fn the_live_state_is_shared_and_equal_only_to_itself() {
        let a = RemoteLive::new(Some("t1".into()), 7);
        let b = a.clone();
        b.set_seq(9);
        b.set_link(LinkState::Live);
        assert_eq!(a.seq(), 9);
        assert_eq!(a.link(), LinkState::Live);
        assert_eq!(a, b);
        assert_ne!(a, RemoteLive::new(Some("t1".into()), 9));
    }

    /// Remote sessions PR 6: file work waits for the link in the order asked; once the link ends, what still waits is
    /// handed back to be failed and anything asked later is failed at once, so no asker waits forever.
    #[test]
    fn errands_wait_in_order_and_none_is_lost_when_the_link_ends() {
        let live = RemoteLive::default();
        let told: Arc<Mutex<Vec<String>>> = Arc::default();
        let put = |name: &str| {
            let told = Arc::clone(&told);
            let name = name.to_string();
            Errand::Put {
                files: vec![Upload {
                    name: name.clone(),
                    data: Vec::new(),
                }],
                done: Box::new(move |r| {
                    lk(&told).push(format!("{name}: {}", r.is_ok()));
                }),
            }
        };
        live.send_errand(put("a"));
        live.send_errand(put("b"));
        let got = live.take_errands();
        assert_eq!(got.len(), 2);
        let names: Vec<String> = got
            .iter()
            .map(|e| match e {
                Errand::Put { files, .. } => files[0].name.clone(),
                Errand::Fetch { .. } => String::new(),
            })
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(live.take_errands().is_empty());
        live.send_errand(put("c"));
        for e in live.end_errands() {
            e.fail(ErrandFail::Failed("ended".into()));
        }
        assert_eq!(live.link(), LinkState::Ended);
        live.send_errand(put("d"));
        assert_eq!(*lk(&told), ["c: false", "d: false"]);
        drop(got);
    }
}
