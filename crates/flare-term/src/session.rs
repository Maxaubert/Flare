//! SessionManager, SessionSink.
//!
//! Every shell is a `Session`: the process, a reader thread, a pump thread (batches output every
//! 8 ms) and a writer thread. A session is born either for a tab or as a warm spare that keeps its
//! banner until a tab adopts it. Nothing here waits on the caller's thread except `spawn` and
//! `prewarm`, which say so.

use crate::batcher::{BATCH_MAX_BYTES, BATCH_WINDOW, OutputBatcher};
use crate::exit_poll::{ExitPoller, PollStep};
use crate::pty::{PtyProcess, PtySpawner, SpawnSpec};
use flare_shells::cwd::cd_command;
use flare_shells::env::{PluginEnv, plugin_key, pty_env};
use flare_shells::list::{ShellDef, shell_by_id};
use flare_shells::resume::{Launch, with_resume};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where a session's output and its exit go. `data` runs under the session's deliver lock, so it
/// must not call back into `flush_output`.
pub trait SessionSink: Send + Sync {
    fn data(&self, id: &str, bytes: Vec<u8>);
    fn exit(&self, id: &str, code: Option<u32>);
}

/// The shells a tab can start. May block (a cold detection): only ever called from a spawn worker.
pub trait ShellSource: Send + Sync {
    fn shells(&self) -> Vec<ShellDef>;
}

pub struct RealShells;

impl ShellSource for RealShells {
    fn shells(&self) -> Vec<ShellDef> {
        flare_shells::detect::cached_shells().to_vec()
    }
}

/// Is this plugin folder OUR Claude plugin? The default reads the folder's `plugin.json`, so
/// EVERY manager drops an inherited copy of ours without its caller remembering (rule 109).
#[derive(Clone)]
pub struct IsOurs(pub Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl Default for IsOurs {
    fn default() -> Self {
        IsOurs(Arc::new(flare_agent::plugin::is_our_plugin))
    }
}

impl std::fmt::Debug for IsOurs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IsOurs")
    }
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub resize_retry_delay: Duration,
    pub resize_retries: u32,
    pub batch_window: Duration,
    pub warm_max: usize,
    pub warm_banner_cap: usize,
    pub exit_grace: Duration,
    pub backlog_max: usize,
    pub is_ours: IsOurs,
    /// The environment a shell inherits; None = the process environment (a test seam).
    pub base_env: Option<Vec<(String, String)>>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            resize_retry_delay: Duration::from_millis(300),
            resize_retries: 5,
            batch_window: BATCH_WINDOW,
            warm_max: 2,
            warm_banner_cap: 65_536,
            exit_grace: Duration::from_millis(100),
            backlog_max: 4 * 1024 * 1024,
            is_ours: IsOurs::default(),
            base_env: None,
        }
    }
}

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Where a session's batches go right now.
enum Dest {
    /// A warm spare: the banner is kept (capped) for the tab that adopts it.
    Buffer(Vec<u8>),
    Sink {
        id: String,
        sink: Arc<dyn SessionSink>,
    },
}

struct IoState {
    batcher: OutputBatcher,
    stop: bool,
    last_read: Instant,
}

#[derive(Default)]
struct Threads {
    reader: Option<JoinHandle<()>>,
    pump: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

struct Session {
    token: u64,
    tab_token: String,
    def_id: String,
    plugin_key: String,
    pid: u32,
    warm_key: Option<String>,
    banner_cap: usize,
    killed: AtomicBool,
    exited: AtomicBool,
    proc: Mutex<Option<Box<dyn PtyProcess>>>,
    state: Mutex<IoState>,
    pump_cv: Condvar,
    space_cv: Condvar,
    /// Held while a batch is taken AND handed over, so batches never reorder.
    dest: Mutex<Dest>,
    /// Set (under the manager's `st` lock) once the finisher reported the exit to a sink, so a
    /// spawn that loses the race with a shell dying at birth never registers it.
    reported: AtomicBool,
    /// Set (under `dest`) when the session ended while it was still a warm spare.
    finished: Mutex<Option<Option<u32>>>,
    writer_tx: Mutex<Option<Sender<Vec<u8>>>>,
    threads: Mutex<Threads>,
}

fn append_capped(buf: &mut Vec<u8>, bytes: &[u8], cap: usize) {
    let room = cap.saturating_sub(buf.len());
    buf.extend_from_slice(&bytes[..room.min(bytes.len())]);
}

impl Session {
    /// Takes what the batcher holds (all of it when `force`, else only when due) and hands it over.
    fn deliver(&self, force: bool) {
        let mut dest = lk(&self.dest);
        if self.killed.load(Ordering::SeqCst) {
            return;
        }
        let chunk = {
            let mut st = lk(&self.state);
            if force {
                st.batcher.flush()
            } else {
                st.batcher.take_due(Instant::now())
            }
        };
        self.space_cv.notify_all();
        if let Some(bytes) = chunk {
            match &mut *dest {
                Dest::Buffer(buf) => append_capped(buf, &bytes, self.banner_cap),
                Dest::Sink { id, sink } => sink.data(id, bytes),
            }
        }
    }

    fn stop_io(&self) {
        lk(&self.state).stop = true;
        self.pump_cv.notify_all();
        self.space_cv.notify_all();
    }

    fn join_threads(&self, which: fn(&mut Threads) -> Option<JoinHandle<()>>) {
        let h = which(&mut lk(&self.threads));
        if let Some(h) = h {
            let _ = h.join(); // sync-ok: a worker thread of the session, ended by closing its pty or channel
        }
    }
}

struct State {
    sessions: HashMap<String, Arc<Session>>,
    pending: HashSet<String>,
    killed_pending: HashSet<String>,
    desired: HashMap<String, (usize, usize)>,
    /// Oldest first; the key is the lower-cased root.
    warm: Vec<(String, Arc<Session>)>,
}

struct Shared {
    spawner: Arc<dyn PtySpawner>,
    shells: Arc<dyn ShellSource>,
    opts: SessionOptions,
    st: Mutex<State>,
    /// Killed shells not yet gone, and spawns or prewarms still in flight.
    dying: Mutex<Counts>,
    dying_cv: Condvar,
    /// Bumped by `kill_all`: a prewarm that started before it must not keep its spare.
    quit_gen: AtomicU64,
    ticks: AtomicU64,
    next_token: AtomicU64,
}

#[derive(Default)]
struct Counts {
    dying: usize,
    starting: usize,
}

pub struct SessionManager {
    sh: Arc<Shared>,
}

/// Counts a spawn or prewarm in flight, so the quit wait also covers a shell that is still being
/// started when the window closes (it is then killed by the `killed_pending` or `quit_gen` check).
struct StartGuard<'a>(&'a Shared);

impl<'a> StartGuard<'a> {
    fn new(sh: &'a Shared) -> Self {
        lk(&sh.dying).starting += 1;
        Self(sh)
    }
}

impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        lk(&self.0.dying).starting -= 1;
        self.0.dying_cv.notify_all();
    }
}

/// Removes the id from `pending` and `killed_pending` on every way out of `spawn`.
struct PendingGuard<'a> {
    sh: &'a Shared,
    id: &'a str,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let mut st = lk(&self.sh.st);
        st.pending.remove(self.id);
        st.killed_pending.remove(self.id);
    }
}

fn reader_loop(
    sh: Arc<Shared>,
    s: Arc<Session>,
    mut reader: Box<dyn Read + Send>,
    backlog_max: usize,
) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        sh.ticks.fetch_add(1, Ordering::SeqCst);
        let mut st = lk(&s.state);
        while st.batcher.len() >= backlog_max && !st.stop {
            st = s.space_cv.wait(st).unwrap_or_else(|p| p.into_inner()); // sync-ok: the reader thread throttles a flooding shell
        }
        if st.stop {
            break;
        }
        let was_empty = st.batcher.is_empty();
        let now = Instant::now();
        st.batcher.push(&buf[..n], now);
        st.last_read = now;
        let big = st.batcher.len() >= BATCH_MAX_BYTES;
        drop(st);
        if was_empty || big {
            s.pump_cv.notify_one();
        }
    }
}

fn pump_loop(s: Arc<Session>) {
    loop {
        {
            let mut st = lk(&s.state);
            loop {
                if st.stop {
                    return;
                }
                match st.batcher.due_in(Instant::now()) {
                    Some(d) if d.is_zero() => break,
                    Some(d) => {
                        st = s
                            .pump_cv
                            .wait_timeout(st, d)
                            .unwrap_or_else(|p| p.into_inner())
                            .0; // sync-ok: the pump thread
                    }
                    None => {
                        st = s.pump_cv.wait(st).unwrap_or_else(|p| p.into_inner()); // sync-ok: the pump thread
                    }
                }
            }
        }
        s.deliver(false);
    }
}

fn writer_loop(rx: std::sync::mpsc::Receiver<Vec<u8>>, mut w: Box<dyn std::io::Write + Send>) {
    // sync-ok: the writer thread of a session, ended by dropping the sender
    while let Ok(bytes) = rx.recv() {
        if w.write_all(&bytes).is_err() {
            break;
        }
        let _ = w.flush();
    }
}

struct Fresh<'a> {
    spec: &'a SpawnSpec,
    def_id: &'a str,
    plugin_key: String,
    tab_token: String,
    warm_key: Option<String>,
    dest: Dest,
}

/// Starts the process and its three threads. Holds no manager lock while it spawns.
fn launch(sh: &Arc<Shared>, f: Fresh<'_>) -> Result<Arc<Session>, String> {
    let mut proc = sh.spawner.spawn(f.spec)?;
    let pid = proc.pid();
    let reader = proc.take_reader().ok_or("the pty has no reader")?;
    let writer = proc.take_writer().ok_or("the pty has no writer")?;
    let (tx, rx) = channel::<Vec<u8>>();
    let s = Arc::new(Session {
        token: sh.next_token.fetch_add(1, Ordering::SeqCst),
        tab_token: f.tab_token,
        def_id: f.def_id.to_string(),
        plugin_key: f.plugin_key,
        pid,
        warm_key: f.warm_key,
        banner_cap: sh.opts.warm_banner_cap,
        killed: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        reported: AtomicBool::new(false),
        proc: Mutex::new(Some(proc)),
        state: Mutex::new(IoState {
            batcher: OutputBatcher::new(sh.opts.batch_window, BATCH_MAX_BYTES),
            stop: false,
            last_read: Instant::now(),
        }),
        pump_cv: Condvar::new(),
        space_cv: Condvar::new(),
        dest: Mutex::new(f.dest),
        finished: Mutex::new(None),
        writer_tx: Mutex::new(Some(tx)),
        threads: Mutex::new(Threads::default()),
    });
    let spawn_err = |e: std::io::Error| {
        s.stop_io();
        lk(&s.proc).take();
        e.to_string()
    };
    let backlog = sh.opts.backlog_max;
    let (sh2, s2) = (Arc::clone(sh), Arc::clone(&s));
    let reader_h = std::thread::Builder::new()
        .name("flare-pty-read".into())
        .spawn(move || reader_loop(sh2, s2, reader, backlog))
        .map_err(spawn_err)?;
    let s2 = Arc::clone(&s);
    let pump_h = std::thread::Builder::new()
        .name("flare-pty-pump".into())
        .spawn(move || pump_loop(s2))
        .map_err(spawn_err)?;
    let writer_h = std::thread::Builder::new()
        .name("flare-pty-write".into())
        .spawn(move || writer_loop(rx, writer))
        .map_err(spawn_err)?;
    *lk(&s.threads) = Threads {
        reader: Some(reader_h),
        pump: Some(pump_h),
        writer: Some(writer_h),
    };
    let (sh3, s3) = (Arc::clone(sh), Arc::clone(&s));
    let cb: Box<dyn FnOnce(Option<u32>) + Send> = Box::new(move |code| {
        s3.exited.store(true, Ordering::SeqCst);
        let (sh4, s4) = (sh3, Arc::clone(&s3));
        let r = std::thread::Builder::new()
            .name("flare-pty-finish".into())
            .spawn(move || finish(&sh4, &s4, code));
        if let Err(e) = r {
            log::warn!("could not start the finisher thread: {e}");
        }
    });
    if let Some(p) = lk(&s.proc).as_mut() {
        p.on_exit(cb);
    }
    Ok(s)
}

/// The shell exited by itself. ConPTY sends no end-of-file, so wait until the output has been quiet
/// for a moment, close the pty (which ends the reader), flush, and only then report the exit.
fn finish(sh: &Arc<Shared>, s: &Arc<Session>, code: Option<u32>) {
    if let Some(key) = &s.warm_key {
        let mut st = lk(&sh.st);
        if let Some(i) = st
            .warm
            .iter()
            .position(|(k, w)| k == key && w.token == s.token)
        {
            st.warm.remove(i);
        }
    }
    if s.killed.load(Ordering::SeqCst) {
        return;
    }
    let started = Instant::now();
    loop {
        let idle = lk(&s.state).last_read.elapsed();
        if idle >= sh.opts.exit_grace || started.elapsed() >= Duration::from_secs(1) {
            break;
        }
        std::thread::sleep((sh.opts.exit_grace - idle).min(Duration::from_millis(20))); // sync-ok: the finisher thread
    }
    drop(lk(&s.proc).take());
    s.join_threads(|t| t.reader.take());
    s.stop_io();
    s.join_threads(|t| t.pump.take());
    {
        let mut dest = lk(&s.dest);
        if !s.killed.load(Ordering::SeqCst) {
            let rest = lk(&s.state).batcher.flush();
            match &mut *dest {
                Dest::Buffer(buf) => {
                    if let Some(b) = rest {
                        append_capped(buf, &b, s.banner_cap);
                    }
                    *lk(&s.finished) = Some(code);
                }
                Dest::Sink { id, sink } => {
                    if let Some(b) = rest {
                        sink.data(id, b);
                    }
                    sink.exit(id, code);
                    let (id, tok) = (id.clone(), s.token);
                    let mut st = lk(&sh.st);
                    s.reported.store(true, Ordering::SeqCst);
                    if st.sessions.get(&id).is_some_and(|x| x.token == tok) {
                        st.sessions.remove(&id);
                        st.desired.remove(&id);
                    }
                }
            }
        }
    }
    *lk(&s.writer_tx) = None;
    s.join_threads(|t| t.writer.take());
}

/// Ends a session for good (the death is ours: no flush, no sink calls). Counted in `dying` until
/// the process is gone and its threads are joined.
fn kill_pty(sh: &Arc<Shared>, s: Arc<Session>) {
    if s.killed.swap(true, Ordering::SeqCst) {
        return;
    }
    lk(&sh.dying).dying += 1;
    let sh2 = Arc::clone(sh);
    let s2 = Arc::clone(&s);
    let r = std::thread::Builder::new()
        .name("flare-pty-kill".into())
        .spawn(move || {
            s2.stop_io();
            // ConPTY asks for the cursor (ESC[6n) and the device attributes (ESC[c) at startup and
            // waits for both answers, on close too: a shell killed before they were answered makes
            // the pseudo console linger 1.5 to 3 s, so the quit wait runs out. The shell is dying,
            // so answer both ourselves (measured 2026-10-17: close 3 s -> under 10 ms).
            if let Some(tx) = lk(&s2.writer_tx).take() {
                let _ = tx.send(b"\x1b[1;1R\x1b[?1;2c".to_vec());
            }
            let proc = lk(&s2.proc).take();
            if let Some(mut p) = proc {
                p.kill();
                let t0 = Instant::now();
                let mut poller = ExitPoller::new();
                loop {
                    match poller.step(t0.elapsed(), p.exit_code().is_some()) {
                        PollStep::Done => break,
                        PollStep::Wait(d) => std::thread::sleep(d), // sync-ok: the killer thread
                        PollStep::Stuck(d) => {
                            log::warn!("kill-stuck: pid {} has not exited", s2.pid);
                            std::thread::sleep(d); // sync-ok: the killer thread
                        }
                    }
                }
                drop(p);
            }
            s2.join_threads(|t| t.reader.take());
            s2.join_threads(|t| t.pump.take());
            s2.join_threads(|t| t.writer.take());
            lk(&sh2.dying).dying -= 1;
            sh2.dying_cv.notify_all();
        });
    if let Err(e) = r {
        log::warn!("could not start the killer thread: {e}");
        lk(&sh.dying).dying -= 1;
        sh.dying_cv.notify_all();
    }
}

impl SessionManager {
    pub fn new(
        spawner: Arc<dyn PtySpawner>,
        shells: Arc<dyn ShellSource>,
        opts: SessionOptions,
    ) -> Arc<Self> {
        Arc::new(Self {
            sh: Arc::new(Shared {
                spawner,
                shells,
                opts,
                st: Mutex::new(State {
                    sessions: HashMap::new(),
                    pending: HashSet::new(),
                    killed_pending: HashSet::new(),
                    desired: HashMap::new(),
                    warm: Vec::new(),
                }),
                dying: Mutex::new(Counts::default()),
                dying_cv: Condvar::new(),
                ticks: AtomicU64::new(0),
                next_token: AtomicU64::new(1),
                quit_gen: AtomicU64::new(0),
            }),
        })
    }

    fn spec_for(
        &self,
        def: &ShellDef,
        root: &str,
        size: (usize, usize),
        resume: Option<&str>,
        plugin: Option<&PluginEnv>,
        token: &str,
    ) -> SpawnSpec {
        let launch = match resume {
            Some(r) => with_resume(def, r),
            None => Launch {
                exe: def.exe.clone(),
                args: def.args.clone(),
                env: None,
            },
        };
        let from: Vec<(String, String)> = match &self.sh.opts.base_env {
            Some(b) => b.clone(),
            None => std::env::vars().collect(),
        };
        let mut env = pty_env(
            &from,
            Some(def.id.as_str()),
            plugin,
            Some(&*self.sh.opts.is_ours.0),
            Some(token),
        );
        if let Some(pair) = launch.env {
            env.push(pair);
        }
        SpawnSpec {
            exe: launch.exe,
            args: launch.args,
            cwd: root.to_string(),
            env,
            cols: size.0.min(u16::MAX as usize) as u16,
            rows: size.1.min(u16::MAX as usize) as u16,
        }
    }

    fn size_for(st: &State, id: &str) -> (usize, usize) {
        st.desired
            .get(id)
            .or_else(|| st.desired.get(""))
            .copied()
            .unwrap_or((80, 24))
    }

    /// BLOCKING (reads the shell list, spawns): call it from a worker thread, never the UI thread.
    /// Returns whether a shell now runs for `id`.
    pub fn spawn(
        &self,
        id: &str,
        root: &str,
        shell_id: Option<&str>,
        sink: Arc<dyn SessionSink>,
        resume: Option<&str>,
        plugin: Option<&PluginEnv>,
    ) -> bool {
        // An invalid id is no resume at all (the length only is logged, never the text).
        let valid = flare_agent::resume::valid_resume(resume);
        if resume.is_some() && valid.is_none() {
            log::warn!(
                "a resume id of {} chars is not a session id; starting a plain shell",
                resume.map_or(0, str::len)
            );
        }
        let resume = valid;
        {
            let mut st = lk(&self.sh.st);
            if st.sessions.contains_key(id) || st.pending.contains(id) {
                return false;
            }
            st.pending.insert(id.to_string());
        }
        let _guard = PendingGuard { sh: &self.sh, id };
        // Declared after the pending guard, so it drops first: a kill made below is counted in
        // `dying` before this spawn stops counting as `starting`.
        let _starting = StartGuard::new(&self.sh);
        let list = self.sh.shells.shells();
        let Some(def) = shell_by_id(shell_id, &list).cloned() else {
            return false;
        };
        let key = root.to_lowercase();
        let pk = plugin_key(plugin);
        // Adopt a warm spare, or launch.
        let size;
        {
            let mut st = lk(&self.sh.st);
            if st.killed_pending.contains(id) {
                return false;
            }
            if resume.is_none()
                && let Some(i) = st.warm.iter().position(|(k, w)| {
                    *k == key
                        && !w.exited.load(Ordering::SeqCst)
                        && w.def_id == def.id
                        && w.plugin_key == pk
                })
            {
                let (_, w) = st.warm.remove(i);
                st.sessions.insert(id.to_string(), Arc::clone(&w));
                let want = st.desired.get(id).copied();
                drop(st);
                self.adopt(&w, id, sink);
                if let Some((c, r)) = want {
                    self.apply_resize(&w, id, c, r);
                }
                return true;
            }
            size = Self::size_for(&st, id);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let spec = self.spec_for(&def, root, size, resume, plugin, &token);
        let fresh = Fresh {
            spec: &spec,
            def_id: &def.id,
            plugin_key: pk,
            tab_token: token,
            warm_key: None,
            dest: Dest::Sink {
                id: id.to_string(),
                sink,
            },
        };
        let s = match launch(&self.sh, fresh) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("could not start {}: {e}", def.exe);
                return false;
            }
        };
        let mut st = lk(&self.sh.st);
        if st.killed_pending.contains(id) {
            drop(st);
            kill_pty(&self.sh, s);
            return false;
        }
        if s.reported.load(Ordering::SeqCst) {
            // The shell already ended and its exit went to the sink: nothing to register.
            return true;
        }
        st.sessions.insert(id.to_string(), Arc::clone(&s));
        let want = st.desired.get(id).copied().filter(|w| *w != size);
        drop(st);
        if let Some((c, r)) = want {
            self.apply_resize(&s, id, c, r);
        }
        true
    }

    /// Hands a warm spare to a tab: its banner first, then the live output.
    fn adopt(&self, w: &Arc<Session>, id: &str, sink: Arc<dyn SessionSink>) {
        let mut dest = lk(&w.dest);
        let mut banner = match std::mem::replace(&mut *dest, Dest::Buffer(Vec::new())) {
            Dest::Buffer(b) => b,
            Dest::Sink { .. } => Vec::new(),
        };
        if let Some(rest) = lk(&w.state).batcher.flush() {
            append_capped(&mut banner, &rest, w.banner_cap);
        }
        w.space_cv.notify_all();
        if !banner.is_empty() {
            sink.data(id, banner);
        }
        let finished = *lk(&w.finished);
        if let Some(code) = finished {
            // The spare ended before the tab took it: the exit is still owed.
            sink.exit(id, code);
            let mut st = lk(&self.sh.st);
            if st.sessions.get(id).is_some_and(|x| x.token == w.token) {
                st.sessions.remove(id);
                st.desired.remove(id);
            }
        }
        *dest = Dest::Sink {
            id: id.to_string(),
            sink,
        };
    }

    /// BLOCKING, best effort, never panics: keeps a started shell ready for the next tab in `root`.
    pub fn prewarm(&self, root: &str, shell_id: Option<&str>, plugin: Option<&PluginEnv>) {
        let _starting = StartGuard::new(&self.sh);
        let gen0 = self.sh.quit_gen.load(Ordering::SeqCst);
        let list = self.sh.shells.shells();
        let Some(def) = shell_by_id(shell_id, &list).cloned() else {
            return;
        };
        let key = root.to_lowercase();
        let pk = plugin_key(plugin);
        let mut victims: Vec<Arc<Session>> = Vec::new();
        let size;
        {
            let mut st = lk(&self.sh.st);
            if let Some(i) = st.warm.iter().position(|(k, _)| *k == key) {
                let w = &st.warm[i].1;
                if w.plugin_key == pk && w.def_id == def.id && !w.exited.load(Ordering::SeqCst) {
                    return;
                }
                victims.push(st.warm.remove(i).1);
            }
            while st.warm.len() >= self.sh.opts.warm_max.max(1) {
                victims.push(st.warm.remove(0).1);
            }
            size = st.desired.get("").copied().unwrap_or((80, 24));
        }
        for v in victims {
            self.dispose(v);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let spec = self.spec_for(&def, root, size, None, plugin, &token);
        let fresh = Fresh {
            spec: &spec,
            def_id: &def.id,
            plugin_key: pk,
            tab_token: token,
            warm_key: Some(key.clone()),
            dest: Dest::Buffer(Vec::new()),
        };
        let Ok(s) = launch(&self.sh, fresh) else {
            return;
        };
        let mut old = None;
        {
            let mut st = lk(&self.sh.st);
            if self.sh.quit_gen.load(Ordering::SeqCst) != gen0 {
                // Everything was killed while this spare started: it must not outlive the quit.
                drop(st);
                self.dispose(s);
                return;
            }
            if let Some(i) = st.warm.iter().position(|(k, _)| *k == key) {
                old = Some(st.warm.remove(i).1);
            }
            st.warm.push((key, Arc::clone(&s)));
        }
        if let Some(o) = old {
            self.dispose(o);
        }
        // A spare that already exited must not keep a slot.
        if s.exited.load(Ordering::SeqCst) {
            let mut st = lk(&self.sh.st);
            st.warm.retain(|(_, w)| w.token != s.token);
        }
    }

    /// Removes nothing from the maps (the caller did); kills the process unless it already exited.
    fn dispose(&self, s: Arc<Session>) {
        if !s.exited.load(Ordering::SeqCst) {
            kill_pty(&self.sh, s);
        }
    }

    pub fn write(&self, id: &str, data: &[u8]) {
        let s = lk(&self.sh.st).sessions.get(id).cloned();
        if let Some(s) = s
            && let Some(tx) = lk(&s.writer_tx).as_ref()
        {
            let _ = tx.send(data.to_vec());
        }
    }

    /// The id of the shell definition the running session was spawned with.
    pub fn shell_of(&self, id: &str) -> Option<String> {
        lk(&self.sh.st).sessions.get(id).map(|s| s.def_id.clone())
    }

    /// Delivers what the batcher holds NOW, on the caller's thread.
    pub fn flush_output(&self, id: &str) {
        let s = lk(&self.sh.st).sessions.get(id).cloned();
        if let Some(s) = s {
            s.deliver(true);
        }
    }

    /// Writes the cd line for that session's shell. Never counts as typing (decision O1).
    pub fn cd(&self, id: &str, path: &str) -> bool {
        let s = lk(&self.sh.st).sessions.get(id).cloned();
        let Some(s) = s else { return false };
        let Some(line) = cd_command(Some(&s.def_id), path) else {
            return false;
        };
        self.write(id, line.as_bytes());
        true
    }

    pub fn resize(&self, id: &str, cols: usize, rows: usize) {
        if cols < 2 || rows < 1 {
            return;
        }
        let s = {
            let mut st = lk(&self.sh.st);
            st.desired.insert(id.to_string(), (cols, rows));
            st.sessions.get(id).cloned()
        };
        if let Some(s) = s {
            self.apply_resize(&s, id, cols, rows);
        }
    }

    /// One try now; on an error, retries on a helper thread while that size is still the wanted one.
    fn apply_resize(&self, s: &Arc<Session>, id: &str, cols: usize, rows: usize) {
        if try_resize(s, cols, rows) {
            return;
        }
        let (sh, s, id) = (Arc::clone(&self.sh), Arc::clone(s), id.to_string());
        let r = std::thread::Builder::new()
            .name("flare-pty-resize".into())
            .spawn(move || {
                for _ in 0..sh.opts.resize_retries {
                    std::thread::sleep(sh.opts.resize_retry_delay); // sync-ok: a helper thread, never the caller's
                    let wanted = {
                        let st = lk(&sh.st);
                        st.desired.get(&id) == Some(&(cols, rows))
                            && st.sessions.get(&id).is_some_and(|x| x.token == s.token)
                    };
                    if !wanted || s.killed.load(Ordering::SeqCst) || try_resize(&s, cols, rows) {
                        return;
                    }
                }
            });
        if let Err(e) = r {
            log::warn!("could not start the resize retry: {e}");
        }
    }

    pub fn kill(&self, id: &str) {
        let s = {
            let mut st = lk(&self.sh.st);
            if st.pending.contains(id) {
                st.killed_pending.insert(id.to_string());
            }
            st.desired.remove(id);
            st.sessions.remove(id)
        };
        if let Some(s) = s {
            kill_pty(&self.sh, s);
        }
    }

    /// Every session and every warm spare.
    pub fn kill_all(&self) {
        let all: Vec<Arc<Session>> = {
            let mut st = lk(&self.sh.st);
            self.sh.quit_gen.fetch_add(1, Ordering::SeqCst);
            let pending: Vec<String> = st.pending.iter().cloned().collect();
            st.killed_pending.extend(pending);
            st.desired.retain(|k, _| k.is_empty());
            let mut v: Vec<Arc<Session>> = st.sessions.drain().map(|(_, s)| s).collect();
            v.extend(st.warm.drain(..).map(|(_, s)| s));
            v
        };
        for s in all {
            kill_pty(&self.sh, s);
        }
    }

    /// Kills the spare for `root`, or every spare.
    pub fn kill_warm(&self, root: Option<&str>) {
        let gone: Vec<Arc<Session>> = {
            let mut st = lk(&self.sh.st);
            match root {
                None => st.warm.drain(..).map(|(_, s)| s).collect(),
                Some(r) => {
                    let key = r.to_lowercase();
                    let (out, keep): (Vec<_>, Vec<_>) =
                        st.warm.drain(..).partition(|(k, _)| *k == key);
                    st.warm = keep;
                    out.into_iter().map(|(_, s)| s).collect()
                }
            }
        };
        for s in gone {
            self.dispose(s);
        }
    }

    pub fn live_pids(&self) -> Vec<(String, u32)> {
        let st = lk(&self.sh.st);
        st.sessions
            .iter()
            .map(|(id, s)| (id.clone(), s.pid))
            .collect()
    }

    pub fn tab_token(&self, id: &str) -> Option<String> {
        lk(&self.sh.st)
            .sessions
            .get(id)
            .map(|s| s.tab_token.clone())
    }

    pub fn shells_dying(&self) -> usize {
        lk(&self.sh.dying).dying
    }

    /// true when every shell we killed is gone and no spawn or prewarm is still in flight;
    /// false when `timeout` ran out.
    pub fn shells_gone(&self, timeout: Duration) -> bool {
        let guard = lk(&self.sh.dying);
        let (g, _) = self
            .sh
            .dying_cv
            .wait_timeout_while(guard, timeout, |n| n.dying + n.starting > 0) // sync-ok: the quit path waits on purpose, with a cap
            .unwrap_or_else(|p| p.into_inner());
        g.dying + g.starting == 0
    }

    /// Bumped once per chunk read from any shell; only goes up.
    pub fn output_ticks(&self) -> u64 {
        self.sh.ticks.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn is_pending(&self, id: &str) -> bool {
        lk(&self.sh.st).pending.contains(id)
    }

    #[cfg(test)]
    fn starting(&self) -> usize {
        lk(&self.sh.dying).starting
    }
}

fn try_resize(s: &Arc<Session>, cols: usize, rows: usize) -> bool {
    let mut p = lk(&s.proc);
    match p.as_mut() {
        // A closed session has nothing left to resize; stop retrying.
        None => true,
        Some(p) => p
            .resize(
                cols.min(u16::MAX as usize) as u16,
                rows.min(u16::MAX as usize) as u16,
            )
            .is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeShells, FakeSpawner};

    fn fast() -> SessionOptions {
        SessionOptions {
            resize_retry_delay: Duration::from_millis(5),
            exit_grace: Duration::from_millis(10),
            ..SessionOptions::default()
        }
    }

    fn mgr_with(
        spawner: &Arc<FakeSpawner>,
        shells: Arc<dyn ShellSource>,
        opts: SessionOptions,
    ) -> Arc<SessionManager> {
        SessionManager::new(spawner.clone(), shells, opts)
    }

    fn mgr(spawner: &Arc<FakeSpawner>) -> Arc<SessionManager> {
        mgr_with(spawner, FakeShells::new(), fast())
    }

    struct NullSink;
    impl SessionSink for NullSink {
        fn data(&self, _: &str, _: Vec<u8>) {}
        fn exit(&self, _: &str, _: Option<u32>) {}
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Ev {
        Data(String, Vec<u8>),
        Exit(String, Option<u32>),
    }

    #[derive(Default)]
    struct RecSink(Mutex<Vec<Ev>>);
    impl RecSink {
        fn events(&self) -> Vec<Ev> {
            lk(&self.0).clone()
        }
        fn bytes(&self) -> Vec<u8> {
            self.events()
                .into_iter()
                .filter_map(|e| match e {
                    Ev::Data(_, b) => Some(b),
                    Ev::Exit(..) => None,
                })
                .flatten()
                .collect()
        }
    }
    impl SessionSink for RecSink {
        fn data(&self, id: &str, bytes: Vec<u8>) {
            lk(&self.0).push(Ev::Data(id.to_string(), bytes));
        }
        fn exit(&self, id: &str, code: Option<u32>) {
            lk(&self.0).push(Ev::Exit(id.to_string(), code));
        }
    }

    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        cond()
    }

    fn settled<T>(h: &JoinHandle<T>) -> bool {
        let end = Instant::now() + Duration::from_millis(100);
        while Instant::now() < end {
            if h.is_finished() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        h.is_finished()
    }

    fn null() -> Arc<dyn SessionSink> {
        Arc::new(NullSink)
    }

    fn spawn_a(m: &SessionManager) -> bool {
        m.spawn("a", "C:\\x", Some("pwsh"), null(), None, None)
    }

    // The PT sessions tests.

    #[test]
    fn shells_gone_resolves_at_once_when_nothing_was_killed() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        let t = Instant::now();
        assert!(m.shells_gone(Duration::from_secs(5)));
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn shells_gone_waits_until_every_killed_shell_has_exited() {
        let sp = FakeSpawner::new();
        sp.set_late_exit(true);
        let m = mgr(&sp);
        assert!(m.spawn("q1", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(m.spawn("q2", "C:\\x", Some("pwsh"), null(), None, None));
        m.kill_all();
        let m2 = Arc::clone(&m);
        let h = std::thread::spawn(move || m2.shells_gone(Duration::from_secs(5)));
        assert!(!settled(&h));
        sp.made()[0].exit(1);
        assert!(!settled(&h));
        sp.made()[1].exit(1);
        assert!(h.join().unwrap());
    }

    #[test]
    fn shells_gone_gives_up_after_the_timeout() {
        let sp = FakeSpawner::new();
        sp.set_late_exit(true);
        let m = mgr(&sp);
        assert!(m.spawn("q3", "C:\\x", Some("pwsh"), null(), None, None));
        m.kill_all();
        let t = Instant::now();
        assert!(!m.shells_gone(Duration::from_millis(5)));
        assert!(t.elapsed() < Duration::from_millis(500));
        sp.made()[0].exit(1);
        assert!(m.shells_gone(Duration::from_secs(3)));
    }

    #[test]
    fn a_tab_closed_while_its_shell_starts_starts_no_shell() {
        let sp = FakeSpawner::new();
        let (shells, opener) = FakeShells::gated();
        let m = mgr_with(&sp, shells, fast());
        let m2 = Arc::clone(&m);
        let h =
            std::thread::spawn(move || m2.spawn("t1", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(wait_until(|| m.is_pending("t1")));
        m.kill("t1");
        opener.open();
        assert!(!h.join().unwrap());
        assert!(wait_until(|| sp.made().iter().all(|f| f.killed())));
        assert!(m.live_pids().iter().all(|(id, _)| id != "t1"));
    }

    #[test]
    fn the_quit_wait_covers_a_shell_still_starting() {
        let sp = FakeSpawner::new();
        let (shells, opener) = FakeShells::gated();
        let m = mgr_with(&sp, shells, fast());
        let m2 = Arc::clone(&m);
        let h =
            std::thread::spawn(move || m2.spawn("t1", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(wait_until(|| m.is_pending("t1")));
        m.kill_all();
        assert!(!m.shells_gone(Duration::from_millis(5)));
        let m3 = Arc::clone(&m);
        let gone = std::thread::spawn(move || m3.shells_gone(Duration::from_secs(5)));
        assert!(!settled(&gone));
        opener.open();
        assert!(!h.join().unwrap());
        assert!(gone.join().unwrap());
        assert!(sp.made().iter().all(|f| f.killed()));
        assert!(m.live_pids().is_empty());
    }

    #[test]
    fn a_prewarm_still_starting_at_the_quit_keeps_no_spare() {
        let sp = FakeSpawner::new();
        let (shells, opener) = FakeShells::gated();
        let m = mgr_with(&sp, shells, fast());
        let m2 = Arc::clone(&m);
        let h = std::thread::spawn(move || m2.prewarm("C:\\home", Some("pwsh"), None));
        assert!(wait_until(|| m.starting() == 1));
        m.kill_all();
        opener.open();
        h.join().unwrap();
        assert!(m.shells_gone(Duration::from_secs(3)));
        assert_eq!(sp.made().len(), 1);
        assert!(sp.made()[0].killed());
    }

    #[test]
    fn a_second_spawn_for_a_pending_id_is_refused() {
        let sp = FakeSpawner::new();
        let (shells, opener) = FakeShells::gated();
        let m = mgr_with(&sp, shells, fast());
        let (m1, m2) = (Arc::clone(&m), Arc::clone(&m));
        let h1 =
            std::thread::spawn(move || m1.spawn("t2", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(wait_until(|| m.is_pending("t2")));
        let h2 =
            std::thread::spawn(move || m2.spawn("t2", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(!h2.join().unwrap());
        opener.open();
        assert!(h1.join().unwrap());
        assert_eq!(sp.made().len(), 1);
    }

    #[test]
    fn closing_an_adopted_tab_keeps_the_next_warm_shell() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        m.prewarm("C:\\home", Some("pwsh"), None);
        assert!(m.spawn("tab", "C:\\home", Some("pwsh"), null(), None, None));
        assert_eq!(sp.made().len(), 1);
        m.prewarm("C:\\home", Some("pwsh"), None);
        assert_eq!(sp.made().len(), 2);
        m.kill("tab");
        sp.made()[0].exit(1);
        assert!(m.spawn("next", "C:\\home", Some("pwsh"), null(), None, None));
        assert_eq!(sp.made().len(), 2);
        assert!(!sp.made()[1].killed());
    }

    #[test]
    fn a_warm_shell_with_the_other_plugin_answer_makes_way() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        let on = PluginEnv {
            dir: "C:\\PT\\claude-plugin".into(),
            on: true,
        };
        let off = PluginEnv {
            dir: "C:\\PT\\claude-plugin".into(),
            on: false,
        };
        m.prewarm("C:\\home", Some("pwsh"), Some(&on));
        m.prewarm("C:\\home", Some("pwsh"), Some(&off));
        assert_eq!(sp.made().len(), 2);
        assert!(wait_until(|| sp.made()[0].killed()));
        assert!(m.spawn("tab", "C:\\home", Some("pwsh"), null(), None, Some(&off)));
        assert_eq!(sp.made().len(), 2);
        m.prewarm("C:\\home", Some("pwsh"), Some(&off));
        m.prewarm("C:\\home", Some("pwsh"), Some(&off));
        assert_eq!(sp.made().len(), 3);
        assert!(!sp.made()[2].killed());
    }

    // New tests.

    fn env_of(sp: &Arc<FakeSpawner>, i: usize, key: &str) -> Option<String> {
        sp.made()[i]
            .spec()
            .env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn with_base(
        sp: &Arc<FakeSpawner>,
        pairs: &[(&str, &str)],
        ours: fn(&str) -> bool,
    ) -> Arc<SessionManager> {
        let base = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        mgr_with(
            sp,
            FakeShells::new(),
            SessionOptions {
                base_env: Some(base),
                is_ours: IsOurs(Arc::new(ours)),
                ..fast()
            },
        )
    }

    #[test]
    fn the_plugin_folder_reaches_a_new_shell_once_after_the_users_own() {
        let key = "CLAUDE_CODE_PLUGIN_DIRS";
        let sp = FakeSpawner::new();
        let m = with_base(&sp, &[(key, "D:\\mine;E:\\too")], |_| false);
        let on = PluginEnv {
            dir: "C:\\F\\claude-plugin".into(),
            on: true,
        };
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), null(), None, Some(&on)));
        assert_eq!(
            env_of(&sp, 0, key).as_deref(),
            Some("D:\\mine;E:\\too;C:\\F\\claude-plugin")
        );
        let off = PluginEnv {
            on: false,
            ..on.clone()
        };
        assert!(m.spawn("b", "C:\\x", Some("pwsh"), null(), None, Some(&off)));
        assert_eq!(env_of(&sp, 1, key).as_deref(), Some("D:\\mine;E:\\too"));
        let sp2 = FakeSpawner::new();
        let m2 = with_base(&sp2, &[], |_| false);
        assert!(m2.spawn("a", "C:\\x", Some("pwsh"), null(), None, Some(&off)));
        assert_eq!(env_of(&sp2, 0, key), None);
    }

    #[test]
    fn an_inherited_copy_of_our_plugin_is_dropped() {
        let key = "CLAUDE_CODE_PLUGIN_DIRS";
        let ours: fn(&str) -> bool = |d| d.ends_with("other-flare\\claude-plugin");
        let sp = FakeSpawner::new();
        let m = with_base(
            &sp,
            &[(key, "D:\\mine;C:\\other-flare\\claude-plugin")],
            ours,
        );
        let on = PluginEnv {
            dir: "C:\\F\\claude-plugin".into(),
            on: true,
        };
        let off = PluginEnv {
            on: false,
            ..on.clone()
        };
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), null(), None, Some(&on)));
        assert_eq!(
            env_of(&sp, 0, key).as_deref(),
            Some("D:\\mine;C:\\F\\claude-plugin")
        );
        assert!(m.spawn("b", "C:\\x", Some("pwsh"), null(), None, Some(&off)));
        assert_eq!(env_of(&sp, 1, key).as_deref(), Some("D:\\mine"));
        let sp2 = FakeSpawner::new();
        let m2 = with_base(&sp2, &[(key, "C:\\other-flare\\claude-plugin")], ours);
        assert!(m2.spawn("a", "C:\\x", Some("pwsh"), null(), None, Some(&off)));
        assert_eq!(env_of(&sp2, 0, key), None);
    }

    #[test]
    fn an_invalid_resume_id_is_no_resume() {
        let sp = FakeSpawner::new();
        let def = ShellDef {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            exe: "pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "-Command".into(), "hook".into()],
        };
        let m = mgr_with(
            &sp,
            FakeShells::with(vec![def]),
            SessionOptions {
                base_env: Some(Vec::new()),
                ..fast()
            },
        );
        assert!(m.spawn("plain", "C:\\x", Some("pwsh"), null(), None, None));
        let bad = ["x; rm -rf", "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f; calc"];
        for (i, b) in bad.iter().enumerate() {
            let tab = format!("bad{i}");
            assert!(m.spawn(&tab, "C:\\x", Some("pwsh"), null(), Some(b), None));
            assert_eq!(sp.made()[i + 1].spec().args, sp.made()[0].spec().args);
            assert_eq!(env_of(&sp, i + 1, "FLARE_RESUME_ID"), None);
        }
        let good = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
        assert!(m.spawn("good", "C:\\x", Some("pwsh"), null(), Some(good), None));
        assert_eq!(env_of(&sp, 3, "FLARE_RESUME_ID").as_deref(), Some(good));
        assert_ne!(sp.made()[3].spec().args, sp.made()[0].spec().args);
    }

    #[test]
    fn a_spawn_for_a_live_id_is_refused() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        assert!(!spawn_a(&m));
        assert_eq!(sp.made().len(), 1);
    }

    #[test]
    fn a_size_asked_for_before_the_shell_existed_is_applied_at_birth() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        m.resize("a", 100, 30);
        assert!(spawn_a(&m));
        let spec = sp.made()[0].spec();
        assert_eq!((spec.cols, spec.rows), (100, 30));
    }

    #[test]
    fn resize_rejects_bad_sizes() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        m.resize("a", 1, 24);
        m.resize("a", 80, 0);
        assert_eq!(sp.made()[0].resize_calls(), 0);
        m.resize("a", 80, 24);
        assert!(wait_until(|| sp.made()[0].resize_calls() == 1));
    }

    #[test]
    fn resize_retries_five_times_while_still_wanted() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        let f = Arc::clone(&sp.made()[0]);
        f.fail_next_resizes(3);
        m.resize("a", 90, 30);
        assert!(wait_until(|| f.resize_calls() == 4));
        assert!(wait_until(|| f.size() == (90, 30)));
        f.fail_next_resizes(100);
        m.resize("a", 91, 30);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(f.resize_calls(), 10);
        // A newer wish stops the old retries.
        let slow = mgr_with(
            &sp,
            FakeShells::new(),
            SessionOptions {
                resize_retry_delay: Duration::from_millis(40),
                ..fast()
            },
        );
        assert!(slow.spawn("c", "C:\\x", Some("pwsh"), null(), None, None));
        let h = Arc::clone(&sp.made()[1]);
        h.fail_next_resizes(100);
        slow.resize("c", 91, 30);
        assert_eq!(h.resize_calls(), 1);
        slow.resize("c", 92, 30);
        std::thread::sleep(Duration::from_millis(400));
        // 1 for 91, 1 for 92, then only 92 keeps retrying (5 more); 91 never retried after the change.
        assert_eq!(h.resize_calls(), 7);
    }

    #[test]
    fn output_is_batched_and_flushed_before_the_exit_message() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        let f = Arc::clone(&sp.made()[0]);
        f.feed(b"a");
        f.feed(b"b");
        f.feed(b"c");
        assert!(wait_until(|| rec.bytes() == b"abc"));
        let datas = rec
            .events()
            .iter()
            .filter(|e| matches!(e, Ev::Data(..)))
            .count();
        assert!(datas <= 2, "{datas} data calls");
        f.feed(b"bye");
        f.exit(0);
        assert!(wait_until(|| {
            rec.events().last() == Some(&Ev::Exit("a".into(), Some(0)))
        }));
        let ev = rec.events();
        assert_eq!(rec.bytes(), b"abcbye");
        assert!(matches!(ev[ev.len() - 2], Ev::Data(..)));
    }

    #[test]
    fn an_exit_without_end_of_file_still_reports() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        sp.made()[0].exit(0);
        assert!(wait_until(|| rec
            .events()
            .contains(&Ev::Exit("a".into(), Some(0)))));
        assert!(wait_until(|| m.live_pids().is_empty()));
    }

    #[test]
    fn the_warm_banner_is_replayed_and_capped() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        m.prewarm("C:\\home", Some("pwsh"), None);
        let mut big = vec![b'x'; 70_000];
        big[0] = b'F';
        sp.made()[0].feed(&big);
        assert!(wait_until(|| m.output_ticks() >= 2));
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("tab", "C:\\home", Some("pwsh"), rec.clone(), None, None));
        let first = rec.events().into_iter().next().expect("a first data");
        let Ev::Data(_, bytes) = first else {
            panic!("first event is not data")
        };
        assert!(bytes.len() <= 65_536);
        assert_eq!(bytes[0], b'F');
    }

    #[test]
    fn a_warm_shell_is_never_adopted_for_a_resume() {
        let sp = FakeSpawner::new();
        let def = ShellDef {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            exe: "pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "-Command".into(), "hook".into()],
        };
        let m = mgr_with(&sp, FakeShells::with(vec![def]), fast());
        m.prewarm("C:\\home", Some("pwsh"), None);
        let id = "5a1d0c2e-1111-4222-8333-94445555a666";
        assert!(m.spawn("tab", "C:\\home", Some("pwsh"), null(), Some(id), None));
        assert_eq!(sp.made().len(), 2);
        assert!(!sp.made()[0].killed());
        let spec = sp.made()[1].spec();
        assert!(spec.args.iter().any(|a| a.contains("claude --resume")));
        assert!(!spec.args.iter().any(|a| a.contains(id)));
        assert!(
            spec.env
                .contains(&("FLARE_RESUME_ID".to_string(), id.to_string()))
        );
    }

    #[test]
    fn at_most_two_warm_shells_one_per_root() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        m.prewarm("C:\\a", Some("pwsh"), None);
        m.prewarm("C:\\b", Some("pwsh"), None);
        m.prewarm("C:\\c", Some("pwsh"), None);
        assert_eq!(sp.made().len(), 3);
        assert!(wait_until(|| sp.made()[0].killed()));
        assert!(!sp.made()[1].killed());
        assert!(!sp.made()[2].killed());
        m.prewarm("C:\\C", Some("pwsh"), None);
        assert_eq!(sp.made().len(), 3);
    }

    #[test]
    fn kill_warm_kills_every_spare() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        m.prewarm("C:\\a", Some("pwsh"), None);
        m.prewarm("C:\\b", Some("pwsh"), None);
        m.kill_warm(None);
        assert!(wait_until(|| sp.made().iter().all(|f| f.killed())));
    }

    fn tab_id_of(f: &crate::fake::FakeState) -> String {
        f.spec()
            .env
            .iter()
            .find(|(k, _)| k == "FLARE_TAB_ID")
            .map(|(_, v)| v.clone())
            .expect("FLARE_TAB_ID")
    }

    #[test]
    fn every_shell_gets_its_own_tab_token() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(m.spawn("b", "C:\\x", Some("pwsh"), null(), None, None));
        let (ta, tb) = (tab_id_of(&sp.made()[0]), tab_id_of(&sp.made()[1]));
        assert_eq!(ta.len(), 32);
        assert!(ta.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(ta, tb);
        assert_eq!(m.tab_token("a"), Some(ta));
        assert_eq!(m.tab_token("b"), Some(tb));
        m.prewarm("C:\\home", Some("pwsh"), None);
        let warm_token = tab_id_of(&sp.made()[2]);
        assert!(m.spawn("tab", "C:\\home", Some("pwsh"), null(), None, None));
        assert_eq!(m.tab_token("tab"), Some(warm_token));
    }

    #[test]
    fn the_environment_is_pty_env_plus_the_resume_variable() {
        let sp = FakeSpawner::new();
        let def = ShellDef {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            exe: "pwsh.exe".into(),
            args: vec!["-Command".into(), "hook".into()],
        };
        let m = mgr_with(&sp, FakeShells::with(vec![def]), fast());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), null(), Some("abc-1234"), None));
        let env = sp.made()[0].spec().env;
        assert!(env.contains(&("TERM".to_string(), "xterm-256color".to_string())));
        assert!(env.contains(&("COLORTERM".to_string(), "truecolor".to_string())));
        assert!(env.contains(&("FLARE_RESUME_ID".to_string(), "abc-1234".to_string())));
    }

    #[test]
    fn a_failed_spawn_registers_nothing() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        sp.fail_next_spawn();
        assert!(!spawn_a(&m));
        assert!(m.live_pids().is_empty());
        assert!(spawn_a(&m));
    }

    #[test]
    fn kill_is_remembered_until_the_exit_lands() {
        let sp = FakeSpawner::new();
        sp.set_late_exit(true);
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        m.kill("a");
        assert_eq!(m.shells_dying(), 1);
        sp.made()[0].exit(1);
        assert!(wait_until(|| m.shells_dying() == 0));
    }

    #[test]
    fn cd_writes_the_line_for_powershell_and_nothing_for_wsl() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        assert!(m.cd("a", "C:\\it's"));
        let f = Arc::clone(&sp.made()[0]);
        let want = b"Set-Location -LiteralPath 'C:\\it''s'\r".to_vec();
        assert!(wait_until(|| f.written() == want));
        let wsl = ShellDef {
            id: "wsl-Ubuntu".into(),
            name: "WSL: Ubuntu".into(),
            exe: "wsl.exe".into(),
            args: vec![],
        };
        let sp2 = FakeSpawner::new();
        let m2 = mgr_with(&sp2, FakeShells::with(vec![wsl]), fast());
        assert!(m2.spawn("w", "C:\\x", Some("wsl-Ubuntu"), null(), None, None));
        assert!(!m2.cd("w", "C:\\x"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(sp2.made()[0].written().is_empty());
        assert!(!m.cd("nope", "C:\\x"));
    }

    #[test]
    fn flush_output_delivers_pending_bytes_now() {
        let sp = FakeSpawner::new();
        let m = mgr_with(
            &sp,
            FakeShells::new(),
            SessionOptions {
                batch_window: Duration::from_secs(30),
                ..fast()
            },
        );
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        let f = Arc::clone(&sp.made()[0]);
        f.feed(b"abc");
        assert!(wait_until(|| m.output_ticks() >= 1));
        m.flush_output("a");
        assert_eq!(rec.bytes(), b"abc");
        f.feed(b"d");
        assert!(wait_until(|| m.output_ticks() >= 2));
        m.flush_output("a");
        assert_eq!(rec.bytes(), b"abcd");
    }

    #[test]
    fn write_reaches_the_pty_through_the_writer_thread() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(spawn_a(&m));
        m.write("a", b"xyz");
        m.write("nope", b"zzz");
        let f = Arc::clone(&sp.made()[0]);
        assert!(wait_until(|| f.written() == b"xyz"));
    }

    #[test]
    fn output_ticks_only_go_up() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert_eq!(m.output_ticks(), 0);
        assert!(spawn_a(&m));
        let f = Arc::clone(&sp.made()[0]);
        f.feed(b"one");
        assert!(wait_until(|| m.output_ticks() >= 1));
        f.feed(b"two");
        assert!(wait_until(|| m.output_ticks() >= 2));
        let seen = m.output_ticks();
        std::thread::sleep(Duration::from_millis(20));
        assert!(m.output_ticks() >= seen);
    }

    #[test]
    fn killing_one_tab_leaves_the_others() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), null(), None, None));
        assert!(m.spawn("b", "C:\\x", Some("pwsh"), null(), None, None));
        m.kill("a");
        let live = m.live_pids();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, "b");
        assert!(!sp.made()[1].killed());
    }

    #[test]
    fn late_output_for_a_closed_tab_goes_nowhere() {
        let sp = FakeSpawner::new();
        let m = mgr(&sp);
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        m.kill("a");
        let f = Arc::clone(&sp.made()[0]);
        f.feed(b"late");
        f.exit(1);
        std::thread::sleep(Duration::from_millis(200));
        assert!(rec.events().is_empty(), "{:?}", rec.events());
    }

    #[test]
    fn a_late_exit_after_the_kill_changes_nothing() {
        let sp = FakeSpawner::new();
        sp.set_late_exit(true);
        let m = mgr(&sp);
        let rec = Arc::new(RecSink::default());
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        m.kill("a");
        assert!(m.spawn("a", "C:\\x", Some("pwsh"), rec.clone(), None, None));
        sp.made()[0].exit(1);
        std::thread::sleep(Duration::from_millis(100));
        let live = m.live_pids();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, "a");
        assert!(rec.events().is_empty());
    }
}
