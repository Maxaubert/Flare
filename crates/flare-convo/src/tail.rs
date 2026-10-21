//! Tails a transcript that is still being written.
//!
//! `Tail` is a plain struct: `poll` reads at most `MAX_READ` bytes from its offset and returns the complete lines in
//! them. A partial last line waits for its newline. A line longer than `MAX_LINE` is skipped and counted. A file that
//! got shorter than the offset, or whose file index changed (a replacement), restarts from byte 0 and says so.
//!
//! `subscribe` runs a `Tail` on a worker thread, polling every `POLL` only while the `Subscription` is alive. Dropping
//! the subscription stops the worker. Nothing here touches the UI thread.

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
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

/// What one `poll` produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
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
    /// Bytes of the line being read, without its newline yet.
    partial: Vec<u8>,
    /// Inside a line too long to keep: bytes are dropped until its newline.
    skipping: bool,
    skipped_long: u64,
}

impl Tail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            file_key: None,
            partial: Vec::new(),
            skipping: false,
            skipped_long: 0,
        }
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
        if replaced || len < self.offset {
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
            self.feed(&chunk, &mut batch.lines);
        }
        batch.more = self.offset < len;
        batch.skipped_long = self.skipped_long;
        Ok(batch)
    }

    fn restart(&mut self) {
        self.offset = 0;
        self.partial.clear();
        self.skipping = false;
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
            self.skipped_long += 1;
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

/// A live view of one transcript. Dropping it stops the worker thread within one sleep slice.
pub struct Subscription {
    rx: Receiver<Batch>,
    stop: Arc<AtomicBool>,
}

impl Subscription {
    /// The next batch that has something in it (lines, a restart, or new skips), without waiting.
    pub fn try_next(&self) -> Option<Batch> {
        self.rx.try_recv().ok()
    }

    /// Waits up to `timeout` for the next batch.
    pub fn next_timeout(&self, timeout: Duration) -> Option<Batch> {
        self.rx.recv_timeout(timeout).ok()
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
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let path = path.into();
    let spawned = std::thread::Builder::new()
        .name("flare-convo-tail".into())
        .spawn(move || run(path, tx, &flag));
    if let Err(e) = spawned {
        log::warn!("convo: tail worker did not start: {e}");
    }
    Subscription { rx, stop }
}

fn run(path: PathBuf, tx: mpsc::Sender<Batch>, stop: &AtomicBool) {
    let mut tail = Tail::new(path);
    let mut reported_skips = 0;
    while !stop.load(Ordering::SeqCst) {
        loop {
            match tail.poll() {
                Ok(batch) => {
                    let news = batch.restarted
                        || !batch.lines.is_empty()
                        || batch.skipped_long != reported_skips;
                    let more = batch.more;
                    reported_skips = batch.skipped_long;
                    if news && tx.send(batch).is_err() {
                        return;
                    }
                    if !more {
                        break;
                    }
                }
                Err(e) => {
                    log::debug!("convo: transcript read failed, retrying: {e}");
                    break;
                }
            }
            if stop.load(Ordering::SeqCst) {
                return;
            }
        }
        sleep_slices(POLL, stop);
    }
}

/// Sleeps for `total` in short slices so a stop is seen quickly.
fn sleep_slices(total: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(25);
    let mut slept = Duration::ZERO;
    while slept < total && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(slice);
        slept += slice;
    }
}
