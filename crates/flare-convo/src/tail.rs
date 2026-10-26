//! Tails a transcript that is still being written.
//!
//! `Tail` is a plain struct: `poll` reads at most `MAX_READ` bytes from its offset and returns the complete lines in
//! them (`skip_to_end` starts it at the end of the file instead, for a reader that wants only what comes next). A
//! partial last line waits for its newline. A line longer than `MAX_LINE` is skipped and counted. A file that
//! got shorter than the offset, whose file index changed (a replacement), or whose last `GUARD` bytes before the offset
//! changed (a rewrite in place that ends longer), restarts from byte 0 and says so.
//!
//! `subscribe` (one file) and `subscribe_session` (the session file plus the subagent files beside it) run the tails on
//! a worker thread, polling every `POLL` only while the `Subscription` is alive. The channel holds `CHANNEL_BATCHES`
//! batches: a slow consumer slows the reads, so memory stays bounded. Dropping the subscription stops the worker.
//! Nothing here touches the UI thread.

use std::fs::File; // sync-ok: the imports of the file tail (no call here)
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::Duration;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

/// The most bytes one `poll` reads.
pub const MAX_READ: usize = 4 * 1024 * 1024;
/// A line longer than this (without its newline) is skipped and counted, never held in memory.
pub const MAX_LINE: usize = 8 * 1024 * 1024;
/// How often the worker polls while a subscriber exists.
pub const POLL: Duration = Duration::from_millis(500);
/// The bytes before the offset that must still match on each poll; a change means the file was rewritten.
pub const GUARD: usize = 64;
/// Batches the worker may queue ahead of the consumer. At `MAX_READ` each, the queue is bounded to a few MB.
pub const CHANNEL_BATCHES: usize = 2;

/// What one `poll` produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
    /// Which file the lines came from: `""` for the session file, the subagent's file name (`agent-<id>.jsonl`) for a
    /// subagent. Always `""` for a batch from `subscribe`.
    pub source: String,
    /// Complete lines, in order, without their line ending.
    pub lines: Vec<String>,
    /// The file was replaced or truncated: the consumer must reset its model before folding `lines`.
    pub restarted: bool,
    /// Bytes remain past what this poll read; poll again without waiting.
    pub more: bool,
    /// Lines skipped for being longer than `MAX_LINE`, so far (cumulative for this tail).
    pub skipped_long: u64,
}

#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    /// The next byte to read from the file.
    offset: u64,
    /// The volume and file index of the file the offset belongs to; a change means a replacement. (Not the creation
    /// time: NTFS tunnelling keeps it when a file is deleted and made again within seconds.)
    file_key: Option<(u32, u64)>,
    /// The last up to `GUARD` bytes before `offset`, as they were read.
    guard: Vec<u8>,
    /// Bytes of the line being read, without its newline yet.
    partial: Vec<u8>,
    /// Inside a line too long to keep: bytes are dropped until its newline.
    skipping: bool,
    /// The line being dropped is the tail of one cut by `skip_to_end`, not a long one: it is not counted.
    resync: bool,
    skipped_long: u64,
}

impl Tail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            file_key: None,
            guard: Vec::new(),
            partial: Vec::new(),
            skipping: false,
            resync: false,
            skipped_long: 0,
        }
    }

    /// Moves the offset to the end of the file as it is now, so only what is written from here on is read. A last
    /// line that has no newline yet is dropped up to its newline (it began before the offset). A missing file leaves
    /// the offset at 0: everything written to it once it exists is new. Runs on a worker.
    pub fn skip_to_end(&mut self) -> std::io::Result<()> {
        self.restart();
        let opened = File::open(&self.path); // sync-ok: runs on the reader's worker
        let mut file = match opened {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.file_key = None;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let len = file.metadata()?.len(); // sync-ok: runs on the reader's worker
        self.file_key = file_key(&file);
        if len == 0 {
            return Ok(());
        }
        let start = len.saturating_sub(GUARD as u64);
        file.seek(SeekFrom::Start(start))?; // sync-ok: runs on the reader's worker
        let mut last = vec![0u8; usize::try_from(len - start).unwrap_or(GUARD)];
        file.read_exact(&mut last)?; // sync-ok: runs on the reader's worker
        self.offset = len;
        if last.last() != Some(&b'\n') {
            self.skipping = true;
            self.resync = true;
        }
        self.guard = last;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn skipped_long(&self) -> u64 {
        self.skipped_long
    }

    /// Reads the next bounded chunk. A missing file is an empty batch (the transcript may not exist yet).
    pub fn poll(&mut self) -> std::io::Result<Batch> {
        let mut batch = Batch {
            skipped_long: self.skipped_long,
            ..Batch::default()
        };
        let opened = File::open(&self.path); // sync-ok: runs on the tail worker
        let mut file = match opened {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(batch),
            Err(e) => return Err(e),
        };
        let len = file.metadata()?.len(); // sync-ok: runs on the tail worker
        let key = file_key(&file);
        let replaced = matches!((self.file_key, key), (Some(a), Some(b)) if a != b);
        if replaced || len < self.offset || (len >= self.offset && !self.guard_holds(&mut file)?) {
            self.restart();
            batch.restarted = true;
        }
        self.file_key = key;
        if len > self.offset {
            let want = usize::try_from(len - self.offset)
                .unwrap_or(usize::MAX)
                .min(MAX_READ);
            file.seek(SeekFrom::Start(self.offset))?; // sync-ok: runs on the tail worker
            let mut chunk = Vec::with_capacity(want);
            (&mut file).take(want as u64).read_to_end(&mut chunk)?; // sync-ok: runs on the tail worker
            self.offset += chunk.len() as u64;
            self.remember(&chunk);
            self.feed(&chunk, &mut batch.lines);
        }
        batch.more = self.offset < len;
        batch.skipped_long = self.skipped_long;
        Ok(batch)
    }

    /// True when the bytes before the offset are still the bytes read there (the file was not rewritten).
    fn guard_holds(&self, file: &mut File) -> std::io::Result<bool> {
        if self.guard.is_empty() {
            return Ok(true);
        }
        let start = self.offset - self.guard.len() as u64;
        file.seek(SeekFrom::Start(start))?; // sync-ok: runs on the tail worker
        let mut now = vec![0u8; self.guard.len()];
        file.read_exact(&mut now)?; // sync-ok: runs on the tail worker
        Ok(now == self.guard)
    }

    /// Keeps the last `GUARD` bytes that end at the offset.
    fn remember(&mut self, chunk: &[u8]) {
        let mut all = std::mem::take(&mut self.guard);
        all.extend_from_slice(chunk);
        let keep_from = all.len().saturating_sub(GUARD);
        self.guard = all.split_off(keep_from);
    }

    fn restart(&mut self) {
        self.offset = 0;
        self.guard.clear();
        self.partial.clear();
        self.skipping = false;
        self.resync = false;
    }

    /// Splits a chunk into lines; the bytes after its last newline stay in `partial`.
    fn feed(&mut self, mut rest: &[u8], out: &mut Vec<String>) {
        while let Some(pos) = rest.iter().position(|&b| b == b'\n') {
            self.append(&rest[..pos]);
            if let Some(line) = self.finish_line() {
                out.push(line);
            }
            rest = &rest[pos + 1..];
        }
        self.append(rest);
    }

    fn append(&mut self, bytes: &[u8]) {
        if self.skipping {
            return;
        }
        if self.partial.len() + bytes.len() > MAX_LINE {
            self.skipping = true;
            self.partial.clear();
            return;
        }
        self.partial.extend_from_slice(bytes);
    }

    fn finish_line(&mut self) -> Option<String> {
        if self.skipping {
            self.skipping = false;
            if !std::mem::take(&mut self.resync) {
                self.skipped_long += 1;
            }
            return None;
        }
        let mut line = std::mem::take(&mut self.partial);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

/// The volume serial and file index of an open file: the identity that survives a rename but not a replacement.
/// `None` when Windows will not say (the caller then cannot see a replacement, and keeps the offset).
fn file_key(file: &File) -> Option<(u32, u64)> {
    // SAFETY: the handle belongs to `file`, which lives across the call; `info` is a plain out-struct that the call
    // fills in full on success and that is read only after a success.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) == 0 {
            return None;
        }
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Some((info.dwVolumeSerialNumber, index))
    }
}

/// A live view of one transcript (and its subagents). Dropping it stops the worker thread within one sleep slice.
pub struct Subscription {
    rx: Receiver<Batch>,
    stop: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
}

impl Subscription {
    /// The next batch that has something in it (lines, a restart, or new skips), without waiting.
    pub fn try_next(&self) -> Option<Batch> {
        self.rx.try_recv().ok()
    }

    /// Waits up to `timeout` for the next batch. For a worker or a test; a UI caller uses `try_next`.
    pub fn next_timeout(&self, timeout: Duration) -> Option<Batch> {
        // sync-ok: a worker or a test waits here on purpose; the UI thread calls try_next
        self.rx.recv_timeout(timeout).ok()
    }

    /// Set once the worker thread has returned. Tests use it to see that a dropped subscription really stops the worker.
    #[doc(hidden)]
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts a worker that tails `path` and sends each batch with news. It polls every `POLL` and keeps going only while
/// the subscription lives: a dropped subscription stops it (by the flag, or by a failed send).
pub fn subscribe(path: impl Into<PathBuf>) -> Subscription {
    spawn_worker(path.into(), None)
}

/// Starts a worker that tails the session file `session` and every `agent-*.jsonl` in `subagents` (the subagent folder
/// of `path::subagent_dir`). Each batch names its file in `source`. A subagent file that appears later is picked up on
/// the next poll.
pub fn subscribe_session(
    session: impl Into<PathBuf>,
    subagents: impl Into<PathBuf>,
) -> Subscription {
    spawn_worker(session.into(), Some(subagents.into()))
}

/// Set on drop of the worker, so `exit_flag` turns true however the worker returns.
struct ExitGuard(Arc<AtomicBool>);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn spawn_worker(main: PathBuf, subagents: Option<PathBuf>) -> Subscription {
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BATCHES);
    let stop = Arc::new(AtomicBool::new(false));
    let exited = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let done = Arc::clone(&exited);
    let spawned = std::thread::Builder::new()
        .name("flare-convo-tail".into())
        .spawn(move || {
            let _exit = ExitGuard(done);
            run(main, subagents, tx, &flag)
        });
    if let Err(e) = spawned {
        log::warn!("convo: tail worker did not start: {e}");
    }
    Subscription { rx, stop, exited }
}

/// One tailed file and what has been reported of it.
struct Slot {
    tail: Tail,
    source: String,
    reported_skips: u64,
}

fn run(main: PathBuf, subagents: Option<PathBuf>, tx: SyncSender<Batch>, stop: &AtomicBool) {
    let mut slots = vec![Slot {
        tail: Tail::new(main),
        source: String::new(),
        reported_skips: 0,
    }];
    while !stop.load(Ordering::SeqCst) {
        if let Some(dir) = &subagents {
            for name in agent_files(dir) {
                if !slots.iter().any(|s| s.source == name) {
                    slots.push(Slot {
                        tail: Tail::new(dir.join(&name)),
                        source: name,
                        reported_skips: 0,
                    });
                }
            }
        }
        for slot in slots.iter_mut() {
            if !drain(slot, &tx, stop) {
                return;
            }
        }
        sleep_slices(POLL, stop);
    }
}

/// Polls one tail until it has no more bytes, sending each batch with news. False when the worker must stop (the
/// consumer is gone, or the stop flag is set).
fn drain(slot: &mut Slot, tx: &SyncSender<Batch>, stop: &AtomicBool) -> bool {
    loop {
        match slot.tail.poll() {
            Ok(mut batch) => {
                let news = batch.restarted
                    || !batch.lines.is_empty()
                    || batch.skipped_long != slot.reported_skips;
                let more = batch.more;
                slot.reported_skips = batch.skipped_long;
                if news {
                    batch.source = slot.source.clone();
                    if !send(tx, stop, batch) {
                        return false;
                    }
                }
                if !more {
                    return true;
                }
            }
            Err(e) => {
                log::debug!("convo: transcript read failed, retrying: {e}");
                return true;
            }
        }
        if stop.load(Ordering::SeqCst) {
            return false;
        }
    }
}

/// Sends one batch, waiting while the channel is full, so a slow consumer slows the reads. False when the consumer is
/// gone or the stop flag is set.
fn send(tx: &SyncSender<Batch>, stop: &AtomicBool, mut batch: Batch) -> bool {
    loop {
        match tx.try_send(batch) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(back)) => {
                if stop.load(Ordering::SeqCst) {
                    return false;
                }
                batch = back;
                sleep_slices(SEND_SLICE, stop);
            }
        }
    }
}

/// The names of the `agent-*.jsonl` files in a subagent folder. A missing folder is none.
fn agent_files(dir: &Path) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        // sync-ok: runs on the tail worker
        return Vec::new();
    };
    listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with("agent-") && name.ends_with(".jsonl")).then_some(name)
        })
        .collect()
}

/// How long the worker waits between tries while the channel is full.
const SEND_SLICE: Duration = Duration::from_millis(25);

/// Sleeps for `total` in short slices so a stop is seen quickly.
fn sleep_slices(total: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(25);
    let mut slept = Duration::ZERO;
    while slept < total && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(slice); // sync-ok: the tail worker's own sleep
        slept += slice;
    }
}
