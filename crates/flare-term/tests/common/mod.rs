#![allow(dead_code)]
//! The one rig every real-pty test uses: a shell in the bundled ConPTY, the emulator it feeds,
//! and a reader thread. Dropping the rig kills the shell and waits for it (the Guard).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use flare_term::emulator::{Emulator, TermEvent};
use flare_term::pty::{PortableSpawner, PtyProcess, PtySpawner, SpawnSpec};

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;
type Events = Arc<Mutex<Vec<TermEvent>>>;

/// Prints the crate's warnings (a shell that could not start says why) on stderr, which the test
/// harness shows when a test fails. Safe to call from every test: only the first call installs it.
pub fn log_warnings() {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Warn
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Stderr = Stderr;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

/// A shell running in a real pseudo console, with its emulator and output reader.
pub struct Rig {
    pub emu: Arc<FairMutex<Emulator>>,
    pub events: Events,
    pty: Option<Box<dyn PtyProcess>>,
    writer: Writer,
    reader: Option<JoinHandle<()>>,
    last_data: Arc<AtomicU64>,
    /// The longest hold of the emulator lock by the reader, on its CPU clock, in nanoseconds (off Windows).
    longest_hold_ns: Arc<AtomicU64>,
    started: Instant,
    pid: u32,
}

/// The calling thread's own CPU time (macOS, Linux: to the microsecond): the work a thread did, without the
/// time a shared runner gave its core to another test. The flood test bounds the parser's hold of the lock
/// and the snapshot's copy with it off Windows.
#[cfg(unix)]
pub fn thread_cpu() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a live timespec the call fills in; the clock id is one every unix has.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "the thread's CPU clock");
    Duration::new(
        u64::try_from(ts.tv_sec).unwrap_or(0),
        u32::try_from(ts.tv_nsec).unwrap_or(0),
    )
}

/// Milliseconds since the rig started, saturating.
fn ms_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Writes every reply back to the pty (in order) and keeps every other event.
fn route(evs: Vec<TermEvent>, writer: &Writer, events: &Events) {
    for ev in evs {
        match ev {
            TermEvent::Reply(bytes) => {
                let mut w = writer.lock().unwrap_or_else(|p| p.into_inner());
                let _ = w.write_all(&bytes);
                let _ = w.flush();
            }
            other => events.lock().unwrap_or_else(|p| p.into_inner()).push(other),
        }
    }
}

impl Rig {
    /// Spawns `exe` through the bundled ConPTY. `env` None means this process's environment.
    pub fn spawn(
        exe: &str,
        args: &[&str],
        cwd: &Path,
        env: Option<Vec<(String, String)>>,
        cols: u16,
        rows: u16,
    ) -> Rig {
        let spec = SpawnSpec {
            exe: exe.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: cwd.to_string_lossy().into_owned(),
            env: env.unwrap_or_else(|| std::env::vars().collect()),
            cols,
            rows,
            remote: None,
        };
        let mut pty = PortableSpawner
            .spawn(&spec)
            .unwrap_or_else(|e| panic!("could not spawn {exe}: {e}"));
        let pid = pty.pid();
        let mut reader = pty.take_reader().expect("the pty has a reader");
        let writer: Writer = Arc::new(Mutex::new(pty.take_writer().expect("the pty has a writer")));
        let emu = Arc::new(FairMutex::new(Emulator::new(
            usize::from(cols),
            usize::from(rows),
        )));
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let last_data = Arc::new(AtomicU64::new(0));
        let longest_hold_ns = Arc::new(AtomicU64::new(0));
        let started = Instant::now();

        let (t_emu, t_writer, t_events, t_last, t_hold) = (
            Arc::clone(&emu),
            Arc::clone(&writer),
            Arc::clone(&events),
            Arc::clone(&last_data),
            Arc::clone(&longest_hold_ns),
        );
        let reader_thread = std::thread::Builder::new()
            .name("rig-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                #[cfg(not(unix))]
                let _ = &t_hold;
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    t_last.store(ms_since(started), Ordering::SeqCst);
                    let evs = {
                        let mut e = t_emu.lock();
                        #[cfg(unix)]
                        let held = thread_cpu();
                        let mut evs = e.feed(&buf[..n]);
                        evs.extend(e.release_sync_if_due(Instant::now()));
                        #[cfg(unix)]
                        {
                            let ns = thread_cpu().saturating_sub(held).as_nanos();
                            t_hold
                                .fetch_max(u64::try_from(ns).unwrap_or(u64::MAX), Ordering::SeqCst);
                        }
                        evs
                    };
                    route(evs, &t_writer, &t_events);
                }
            })
            .expect("the reader thread starts");

        Rig {
            emu,
            events,
            pty: Some(pty),
            writer,
            reader: Some(reader_thread),
            last_data,
            longest_hold_ns,
            started,
            pid,
        }
    }

    /// The longest the reader has held the emulator lock, on its CPU clock (off Windows; zero there).
    pub fn longest_hold(&self) -> Duration {
        Duration::from_nanos(self.longest_hold_ns.load(Ordering::SeqCst))
    }

    /// The shell's process id (0 once the rig has been killed).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Lets a due synchronized update go, as the app would, and routes what that produced.
    fn pump_sync(&self) {
        let evs = self.emu.lock().release_sync_if_due(Instant::now());
        route(evs, &self.writer, &self.events);
    }

    /// Raw bytes to the pty (no key encoding).
    pub fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        w.write_all(bytes).expect("the pty accepts input");
        w.flush().expect("the pty flushes input");
    }

    /// Raw text to the pty (no key encoding).
    pub fn type_str(&self, s: &str) {
        self.write(s.as_bytes());
    }

    /// Resizes the emulator first, then the pty: the order `TermHandle` uses.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.emu.lock().resize(usize::from(cols), usize::from(rows));
        let pty = self.pty.as_mut().expect("the shell is still running");
        pty.resize(cols, rows).expect("the pty resizes");
    }

    /// Returns once no byte has arrived for `ms` milliseconds (gives up after 15 s).
    pub fn wait_quiet(&self, ms: u64) {
        let cap = Instant::now() + Duration::from_secs(15);
        loop {
            self.pump_sync();
            let quiet_for =
                ms_since(self.started).saturating_sub(self.last_data.load(Ordering::SeqCst));
            if quiet_for >= ms || Instant::now() >= cap {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// True once `pred` holds for the emulator, or false after `timeout_ms`.
    pub fn wait_for(&self, timeout_ms: u64, pred: impl Fn(&Emulator) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            self.pump_sync();
            if pred(&self.emu.lock()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The text of every screen row.
    pub fn screen(&self) -> Vec<String> {
        self.pump_sync();
        let e = self.emu.lock();
        (0..e.rows())
            .map(|r| e.row_text(i32::try_from(r).unwrap_or(0)))
            .collect()
    }

    /// Screen rows joined across WRAPLINE, trailing blanks trimmed, empty lines dropped.
    pub fn logical_lines(&self) -> Vec<String> {
        self.pump_sync();
        let e = self.emu.lock();
        let last_col = e.cols().saturating_sub(1);
        let mut out = Vec::new();
        let mut cur = String::new();
        for r in 0..e.rows() {
            let line = i32::try_from(r).unwrap_or(0);
            cur.push_str(&e.row_text(line));
            let wrapped = e.term().grid()[Point::new(Line(line), Column(last_col))]
                .flags
                .contains(Flags::WRAPLINE);
            if !wrapped {
                let done = std::mem::take(&mut cur);
                if !done.is_empty() {
                    out.push(done);
                }
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// The cursor as (line, column) on the screen.
    pub fn cursor(&self) -> (i32, usize) {
        self.emu.lock().cursor()
    }

    /// Lines in the scrollback.
    pub fn history(&self) -> usize {
        self.emu.lock().history_size()
    }

    /// None while the shell runs.
    pub fn exit_code(&mut self) -> Option<u32> {
        self.pty.as_mut().and_then(|p| p.exit_code())
    }

    /// Kills the shell, waits at most 5 s for its exit code, closes the pty and joins the reader.
    /// Safe to call twice.
    pub fn kill_and_wait(&mut self) {
        if let Some(p) = self.pty.as_mut() {
            p.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.exit_code().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Dropping the pty ends the process if it is still there, and closes the pseudo console,
        // which is what ends the reader's blocked read.
        self.pty = None;
        if let Some(h) = self.reader.take() {
            let _ = h.join(); // sync-ok: the pty is closed above, so the reader's read has ended
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

/// A folder named `name` inside a fresh system temp folder. The whole temp folder is removed on drop.
pub struct NamedDir {
    _base: tempfile::TempDir,
    path: PathBuf,
}

impl NamedDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A temp dir whose folder name is `name` (its parent is unique to this call).
pub fn temp_dir_named(name: &str) -> NamedDir {
    let base = tempfile::tempdir().expect("a temp folder can be made");
    let path = base.path().join(name);
    std::fs::create_dir_all(&path).expect("the named folder can be made");
    NamedDir { _base: base, path }
}

/// True while process `pid` runs (its exit code is still STILL_ACTIVE).
#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess takes no pointers. A null handle means the process is gone (or not ours
    // to query), so it is not alive.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        // SAFETY: `handle` is a live handle we just opened, and `code` is a valid out-pointer.
        let ok = GetExitCodeProcess(handle, &mut code);
        // SAFETY: `handle` was opened above and is closed exactly once, here.
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// True while process `pid` runs (macOS: it exists and is not a zombie).
#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    flare_proc::alive(pid)
}
