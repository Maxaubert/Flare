//! FakeSpawner and FakePty (test doubles, always compiled).

use crate::pty::{PtyProcess, PtySpawner, SpawnSpec};
use crate::session::ShellSource;
use flare_shells::list::ShellDef;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

type ExitCb = Box<dyn FnOnce(Option<u32>) + Send>;

/// What the tests can see and drive of one fake process.
pub struct FakeState {
    spec: SpawnSpec,
    killed: AtomicBool,
    written: Mutex<Vec<u8>>,
    size: Mutex<(u16, u16)>,
    resize_calls: AtomicUsize,
    fail_resizes: AtomicUsize,
    exit: Mutex<Option<u32>>,
    exit_cb: Mutex<Option<ExitCb>>,
    reader_tx: Mutex<Option<Sender<Vec<u8>>>>,
    late_exit: Arc<AtomicBool>,
    /// While set, the pty's write blocks (a full pipe), until `unblock_writes`.
    write_gate: Gate,
}

impl FakeState {
    /// The next writes block inside the pty, as a full pipe would.
    pub fn block_writes(&self) {
        *lk(&self.write_gate.0) = true;
    }

    /// Lets blocked writes through.
    pub fn unblock_writes(&self) {
        *lk(&self.write_gate.0) = false;
        self.write_gate.1.notify_all();
    }

    pub fn spec(&self) -> SpawnSpec {
        self.spec.clone()
    }

    pub fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    pub fn written(&self) -> Vec<u8> {
        lk(&self.written).clone()
    }

    pub fn size(&self) -> (u16, u16) {
        *lk(&self.size)
    }

    pub fn resize_calls(&self) -> usize {
        self.resize_calls.load(Ordering::SeqCst)
    }

    /// The next `n` resize calls return an error.
    pub fn fail_next_resizes(&self, n: usize) {
        self.fail_resizes.store(n, Ordering::SeqCst);
    }

    /// The process "prints".
    pub fn feed(&self, bytes: &[u8]) {
        if let Some(tx) = lk(&self.reader_tx).as_ref() {
            let _ = tx.send(bytes.to_vec());
        }
    }

    /// The reader sees end-of-file.
    pub fn close_reader(&self) {
        *lk(&self.reader_tx) = None;
    }

    /// Sets the exit code and runs the on_exit callback (once).
    pub fn exit(&self, code: u32) {
        let cb = {
            let mut e = lk(&self.exit);
            if e.is_some() {
                return;
            }
            *e = Some(code);
            lk(&self.exit_cb).take()
        };
        if let Some(cb) = cb {
            cb(Some(code));
        }
    }
}

/// Reads what `FakeState::feed` sent; a closed channel reads 0 (end-of-file).
struct FakeReader {
    rx: Receiver<Vec<u8>>,
    left: Vec<u8>,
}

impl Read for FakeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left.is_empty() {
            match self.rx.recv() {
                // sync-ok: the reader thread of a session, ended by closing the channel
                Ok(v) => self.left = v,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.left.len());
        buf[..n].copy_from_slice(&self.left[..n]);
        self.left.drain(..n);
        Ok(n)
    }
}

struct FakeWriter(Arc<FakeState>);

impl Write for FakeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (m, cv) = &*self.0.write_gate;
        let mut blocked = lk(m);
        while *blocked {
            blocked = cv.wait(blocked).unwrap_or_else(|p| p.into_inner()); // sync-ok: a fake pty write held by a test
        }
        drop(blocked);
        lk(&self.0.written).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct FakePty {
    state: Arc<FakeState>,
    pid: u32,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
}

impl PtyProcess for FakePty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        self.state.resize_calls.fetch_add(1, Ordering::SeqCst);
        let failing = self
            .state
            .fail_resizes
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if failing {
            return Err("fake resize failure".to_string());
        }
        *lk(&self.state.size) = (cols, rows);
        Ok(())
    }

    fn kill(&mut self) {
        self.state.killed.store(true, Ordering::SeqCst);
        if !self.state.late_exit.load(Ordering::SeqCst) {
            self.state.exit(1);
        }
    }

    fn exit_code(&mut self) -> Option<u32> {
        *lk(&self.state.exit)
    }

    fn on_exit(&mut self, cb: ExitCb) {
        let code = {
            let e = lk(&self.state.exit);
            match *e {
                Some(c) => c,
                None => {
                    *lk(&self.state.exit_cb) = Some(cb);
                    return;
                }
            }
        };
        cb(Some(code));
    }
}

impl Drop for FakePty {
    fn drop(&mut self) {
        // Dropping a real pty closes the pseudo console, which ends the reader.
        self.state.close_reader();
    }
}

/// Makes fake processes and remembers each.
pub struct FakeSpawner {
    made: Mutex<Vec<Arc<FakeState>>>,
    late_exit: Arc<AtomicBool>,
    fail_next: AtomicBool,
}

impl FakeSpawner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            made: Mutex::new(Vec::new()),
            late_exit: Arc::new(AtomicBool::new(false)),
            fail_next: AtomicBool::new(false),
        })
    }

    pub fn made(&self) -> Vec<Arc<FakeState>> {
        lk(&self.made).clone()
    }

    /// true: kill() does NOT exit the process by itself; false: kill() exits at once with code 1.
    pub fn set_late_exit(&self, late: bool) {
        self.late_exit.store(late, Ordering::SeqCst);
    }

    pub fn fail_next_spawn(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

impl PtySpawner for FakeSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err("fake spawn failure".to_string());
        }
        let (tx, rx) = channel();
        let state = Arc::new(FakeState {
            spec: spec.clone(),
            killed: AtomicBool::new(false),
            written: Mutex::new(Vec::new()),
            size: Mutex::new((spec.cols, spec.rows)),
            resize_calls: AtomicUsize::new(0),
            fail_resizes: AtomicUsize::new(0),
            exit: Mutex::new(None),
            exit_cb: Mutex::new(None),
            reader_tx: Mutex::new(Some(tx)),
            late_exit: Arc::clone(&self.late_exit),
            write_gate: Arc::new((Mutex::new(false), Condvar::new())),
        });
        let pid = {
            let mut made = lk(&self.made);
            made.push(Arc::clone(&state));
            1000 + (made.len() as u32 - 1)
        };
        Ok(Box::new(FakePty {
            reader: Some(Box::new(FakeReader {
                rx,
                left: Vec::new(),
            })),
            writer: Some(Box::new(FakeWriter(Arc::clone(&state)))),
            state,
            pid,
        }))
    }
}

type Gate = Arc<(Mutex<bool>, Condvar)>;

/// A shell list for tests.
pub struct FakeShells {
    defs: Vec<ShellDef>,
    gate: Option<Gate>,
}

fn pwsh_def() -> ShellDef {
    ShellDef {
        id: "pwsh".to_string(),
        name: "PowerShell 7".to_string(),
        exe: "pwsh.exe".to_string(),
        args: Vec::new(),
    }
}

impl FakeShells {
    pub fn new() -> Arc<Self> {
        Self::with(vec![pwsh_def()])
    }

    pub fn with(defs: Vec<ShellDef>) -> Arc<Self> {
        Arc::new(Self { defs, gate: None })
    }

    /// shells() BLOCKS every caller until `Opener::open` (a latch: once open it stays open).
    pub fn gated() -> (Arc<Self>, Opener) {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        let s = Arc::new(Self {
            defs: vec![pwsh_def()],
            gate: Some(Arc::clone(&gate)),
        });
        (s, Opener(gate))
    }
}

impl ShellSource for FakeShells {
    fn shells(&self) -> Vec<ShellDef> {
        if let Some(gate) = &self.gate {
            let mut open = lk(&gate.0);
            while !*open {
                open = gate.1.wait(open).unwrap_or_else(|p| p.into_inner()); // sync-ok: a spawn worker, waiting for the gate
            }
        }
        self.defs.clone()
    }
}

pub struct Opener(Gate);

impl Opener {
    pub fn open(&self) {
        *lk(&self.0.0) = true;
        self.0.1.notify_all();
    }
}
