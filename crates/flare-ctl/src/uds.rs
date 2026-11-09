//! The control channel on macOS: a Unix domain socket with the named pipe's protocol and guarantees
//! (rules 243 and 249). Same framing, same naming (FNV-1a of the user and the scope), same exported names, so
//! the app, `flare-mcp` and mods use one API on both systems.
//!
//! - **Where**: `<per-user temp folder>/flare-ctl/<16 hex>[-<8 hex>].sock`. The temp folder is the
//!   one the system gives this user (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, mode 0700); `flare-ctl` in
//!   it is made mode 0700 and must be owned by this user with no group or other bits, else the
//!   server refuses to start.
//! - **Who may connect**: the socket is mode 0600; on accept the server reads the client's uid
//!   (`getpeereid`) and drops any other user before reading a byte (the Windows DACL's job).
//! - **Client check**: before sending a byte the client reads the server's pid (`LOCAL_PEERPID`, the
//!   Windows `GetNamedPipeServerProcessId`) and uid (`getpeereid`): another user, or (when the shell
//!   was told which process serves the channel, `FLARE_CTL_PID`) any other process, is refused.
//! - **First instance**: the server takes an exclusive `flock` on `<socket>.lock` beside the socket and
//!   holds it for its whole life; a start that cannot take it fails (as `FILE_FLAG_FIRST_PIPE_INSTANCE`
//!   does), whatever the socket answers (a live server with a full backlog refuses a connect just as a
//!   dead one does). Only while holding the lock is a socket already there removed as the dead one of a
//!   Flare that crashed (the kernel drops the lock with the process). Stopping removes the socket only
//!   while it is still the one this server bound, still under the lock.
//! - **Busy server**: a client whose connect is refused while the socket is there and its lock is held
//!   (a full backlog) waits and retries within its timeout, as for a busy pipe; a socket whose lock is
//!   free is a dead one and fails at once.
//! - **Reading**: a probe (`recv` with `MSG_DONTWAIT`) and short naps, never a parked read; writes block
//!   and a stop shuts the socket down, which fails a write stuck on a client that never reads (the
//!   Windows `CancelIoEx`). `SO_NOSIGPIPE` keeps a write to a gone client an error, not a signal.

use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::{Client, ClientError};
use crate::framing::{Wire, write_line};
use crate::proto::{CtlError, MAX_LINE, Response, code, to_line};
use crate::session::{ConnHandle, Dispatcher, ServerConfig, Shared, serve_connection};

/// Environment variable naming the channel (a socket path here). Flare sets it in the shells it starts
/// (with `FLARE_TAB_ID`), so a child finds the right Flare even when the owner runs two profiles.
pub const ENV_PIPE: &str = "FLARE_CTL_PIPE";

/// How long `Server::stop` waits for workers to leave.
const STOP_WAIT: Duration = Duration::from_secs(2);
/// The longest socket path macOS takes (`sun_path` is 104 bytes with its NUL).
pub const MAX_PATH: usize = 103;
/// The folder under the per-user temp folder that holds the sockets.
const DIR_NAME: &str = "flare-ctl";

fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.to_lowercase().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The per-user temp folder as the system names it (`/var/folders/.../T/`, mode 0700), else `$TMPDIR`.
/// Public for the e2e test hook's socket, which lives in the same private folder (rule 334).
pub fn user_temp_dir() -> PathBuf {
    let mut buf = vec![0u8; 1024];
    // SAFETY: the buffer is writable for its stated length; confstr writes a NUL-terminated string of
    // at most that many bytes and returns the length it needed.
    let need = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if need > 1 && need <= buf.len() {
        buf.truncate(need - 1);
        if let Ok(s) = String::from_utf8(buf) {
            return PathBuf::from(s);
        }
    }
    std::env::temp_dir()
}

/// The folder that holds this user's sockets (not made here; `Server::start` makes it).
pub fn socket_dir() -> PathBuf {
    user_temp_dir().join(DIR_NAME)
}

/// The file name of the socket for `id` (the uid) and `scope`, the same hashes as the Windows pipe
/// name: `<16 hex of FNV-1a(id)>[-<8 hex of FNV-1a(scope)>].sock`.
pub fn socket_file_name(id: &str, scope: Option<&str>) -> String {
    match scope {
        None => format!("{:016x}.sock", fnv1a(id)),
        Some(s) => format!("{:016x}-{:08x}.sock", fnv1a(id), fnv1a(s) as u32),
    }
}

/// The socket path for `id` (the uid) and `scope` (a profile folder, an e2e run): two profiles of one
/// user never share a socket.
pub fn pipe_name(id: &str, scope: Option<&str>) -> String {
    socket_dir()
        .join(socket_file_name(id, scope))
        .to_string_lossy()
        .into_owned()
}

/// This process's effective uid, the identity every check compares.
pub fn own_uid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { libc::geteuid() }
}

/// Who this process runs as: the uid, as text (the Windows SID's place).
pub fn user_sid() -> Option<String> {
    Some(own_uid().to_string())
}

/// The socket of the current user (and `scope`).
pub fn user_pipe_name(scope: Option<&str>) -> Option<String> {
    user_sid().map(|id| pipe_name(&id, scope))
}

/// The channel a client should use: `FLARE_CTL_PIPE` when set and non-empty, else the user's default.
pub fn default_pipe_name() -> Option<String> {
    match std::env::var(ENV_PIPE) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => user_pipe_name(None),
    }
}

/// The one rule both ends apply to the peer's uid: only this user, and an unreadable uid is refused.
pub fn peer_allowed(me: u32, peer: Option<u32>) -> bool {
    peer == Some(me)
}

/// The peer's effective uid as the kernel says (`getpeereid`); None when it cannot be read.
pub fn peer_uid(fd: RawFd) -> Option<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a socket fd and two out-values that outlive the call.
    let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    (r == 0).then_some(uid)
}

/// The pid of the process at the other end of a connected Unix socket (`LOCAL_PEERPID`).
pub fn peer_pid(fd: RawFd) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = libc::socklen_t::try_from(std::mem::size_of::<libc::pid_t>()).ok()?;
    // SAFETY: a socket fd; the out-buffer is a pid_t of `len` bytes.
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut len,
        )
    };
    if r != 0 {
        return None;
    }
    u32::try_from(pid).ok().filter(|p| *p > 0)
}

/// A write to a peer that is gone fails with EPIPE instead of raising SIGPIPE.
fn no_sigpipe(fd: RawFd) {
    let one: libc::c_int = 1;
    // SAFETY: a socket fd; the option value is a c_int that outlives the call.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&raw const one).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

/// One end of a connected socket.
pub struct PipeEnd(UnixStream);

impl PipeEnd {
    fn new(stream: UnixStream) -> PipeEnd {
        no_sigpipe(stream.as_raw_fd());
        PipeEnd(stream)
    }
}

impl Wire for PipeEnd {
    fn poll_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if buf.is_empty() {
            return Ok(Some(0));
        }
        loop {
            // SAFETY: a live socket fd and a writable buffer of `buf.len()` bytes. MSG_DONTWAIT makes
            // this one call non-blocking without changing the socket's mode (writes stay blocking).
            let n = unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n > 0 {
                return Ok(Some(n as usize));
            }
            if n == 0 {
                // The peer closed and everything it wrote has been read.
                return Ok(None);
            }
            let e = io::Error::last_os_error();
            return match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => Ok(Some(0)),
                io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::NotConnected => Ok(None),
                _ => Err(e),
            };
        }
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        (&self.0).write_all(bytes)
    }
}

/// Makes `dir` (mode 0700) when it is missing and checks it is this user's alone: owned by `me`,
/// a real folder (not a link), no group or other bits.
pub fn ensure_private_dir(dir: &Path, me: u32) -> io::Result<()> {
    // sync-ok: startup of the channel server, a local mkdir
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?; // sync-ok: startup, a local stat
    check_private_dir(meta.is_dir(), meta.uid(), meta.mode(), me).map_err(|why| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: {why}", dir.display()),
        )
    })
}

/// The folder rule as a pure function: a folder, owned by `me`, no group or other permission bits.
pub fn check_private_dir(is_dir: bool, owner: u32, mode: u32, me: u32) -> Result<(), &'static str> {
    if !is_dir {
        return Err("the socket folder is not a folder");
    }
    if owner != me {
        return Err("the socket folder belongs to another user");
    }
    if mode & 0o077 != 0 {
        return Err("the socket folder is open to other users");
    }
    Ok(())
}

/// The lock file that says a server is alive on `socket`: `<socket>.lock`, beside it.
pub fn lock_path(socket: &Path) -> PathBuf {
    let mut p = socket.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

/// Opens `path` (made mode 0600 when missing, never through a link, closed on exec) for an `flock`.
fn open_lock(path: &Path, create: bool) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .mode(0o600);
    if create {
        o.write(true).create(true).truncate(false);
    }
    o.open(path) // sync-ok: startup of the channel server or a client's connect, a local open
}

/// `flock` without waiting: Ok(true) when taken, Ok(false) when another open file holds it.
fn try_flock(file: &std::fs::File, op: libc::c_int) -> io::Result<bool> {
    loop {
        // SAFETY: a live fd and a plain operation value.
        if unsafe { libc::flock(file.as_raw_fd(), op | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => {}
            Some(libc::EWOULDBLOCK) => return Ok(false),
            _ => return Err(e),
        }
    }
}

/// How long a start keeps trying for the lock, so a client that holds it shared for an instant (to
/// tell a dead socket from a busy one) never fails it.
const LOCK_TRIES: Duration = Duration::from_millis(250);

/// Takes the server's lock for `socket`, or fails with AddrInUse while another server holds it. A lock
/// taken on a file that was unlinked meanwhile (a server that stopped and removed it) is let go and
/// taken again on the file now at the path, so two servers never each hold a lock of their own.
fn take_server_lock(socket: &Path) -> io::Result<std::fs::File> {
    let path = lock_path(socket);
    let started = Instant::now();
    loop {
        let file = open_lock(&path, true)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != own_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not this user's lock file", path.display()),
            ));
        }
        if try_flock(&file, libc::LOCK_EX)? {
            // sync-ok: startup, a local stat
            let now = std::fs::symlink_metadata(&path).ok();
            if now.is_some_and(|m| (m.dev(), m.ino()) == (meta.dev(), meta.ino())) {
                return Ok(file);
            }
            continue;
        }
        if started.elapsed() >= LOCK_TRIES {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another Flare already serves this control channel",
            ));
        }
        std::thread::sleep(Duration::from_millis(10)); // sync-ok: startup, bounded by LOCK_TRIES
    }
}

/// Whether a server holds the lock of `socket` now: Some(true) alive, Some(false) the lock is free (or
/// there is none, so the socket is a dead one), None when it cannot be told. The shared lock it takes to
/// find out is let go at once.
pub fn server_alive(socket: &Path) -> Option<bool> {
    let file = match open_lock(&lock_path(socket), false) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    let free = try_flock(&file, libc::LOCK_SH).ok()?;
    // Closing the file lets the shared lock go.
    Some(!free)
}

/// Binds `path` for this user only, holding the lock `lock` (see [`take_server_lock`]): a socket already
/// there is the dead one of a Flare that did not quit cleanly and is removed first; anything else in
/// the way is an error.
fn bind_socket(path: &Path, me: u32) -> io::Result<(UnixListener, (u64, u64), std::fs::File)> {
    if path.as_os_str().len() > MAX_PATH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the control socket path is too long: {}", path.display()),
        ));
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the control socket has no folder"))?;
    ensure_private_dir(dir, me)?;
    // Every Flare on this name takes this lock first, so holding it means no live server is there.
    let lock = take_server_lock(path)?;
    // sync-ok: startup of the channel server, a local stat
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} is in the way of the control socket", path.display()),
            ));
        }
        // A process that serves the name without the lock (not a Flare) is still never replaced.
        // sync-ok: startup, a local connect that never waits on the peer
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another process already serves this control channel",
            ));
        }
        // sync-ok: startup, removes the dead socket of a Flare that did not quit cleanly
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    // Accept never parks: a client that left between `poll` and `accept` is a WouldBlock, not a hang.
    listener.set_nonblocking(true)?;
    // The folder is this user's alone already; the socket is too.
    // sync-ok: startup, a local chmod
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let meta = std::fs::symlink_metadata(path)?; // sync-ok: startup, a local stat
    Ok((listener, (meta.dev(), meta.ino()), lock))
}

/// Binds `path` as [`Server::start`] does (a 0700 folder of this user's, the socket 0600, the
/// first-instance lock) for another line protocol: the `--e2e` test hook (rule 334). The listener
/// does not block; the lock file must be kept open for as long as the socket serves.
pub fn bind_private_socket(path: &Path) -> io::Result<(UnixListener, std::fs::File)> {
    let (listener, _id, lock) = bind_socket(path, own_uid())?;
    Ok((listener, lock))
}

/// The running server. Dropping it stops it.
pub struct Server {
    shared: Arc<Shared>,
    name: String,
    accept: Option<JoinHandle<()>>,
    active: Arc<AtomicUsize>,
    /// Writing a byte here wakes the accept thread's `poll`.
    wake: UnixStream,
    /// The device and inode of the socket this server bound, so stop removes only its own.
    bound: (u64, u64),
    /// The exclusive lock on `<socket>.lock`, held for the server's life (dropped after the socket is
    /// removed, so no other start can take the name while this one still serves it).
    _lock: std::fs::File,
}

impl Server {
    /// Binds the socket `name` for the current user and starts accepting. Fails when a live server
    /// holds the name already (another Flare, or a squatter), or the folder is not this user's alone.
    pub fn start(
        name: &str,
        config: ServerConfig,
        dispatcher: Arc<dyn Dispatcher>,
    ) -> io::Result<Server> {
        Self::start_as(name, config, dispatcher, own_uid())
    }

    /// `start`, believing it runs as `uid` when it checks its clients: a test seam that makes every
    /// real client a foreign user, so the refusal of another user is proven on the live socket. The
    /// folder is still checked against this process's own uid.
    #[doc(hidden)]
    pub fn start_as(
        name: &str,
        config: ServerConfig,
        dispatcher: Arc<dyn Dispatcher>,
        uid: u32,
    ) -> io::Result<Server> {
        let (listener, bound, lock) = bind_socket(Path::new(name), own_uid())?;
        let (wake, wake_rx) = UnixStream::pair()?;
        wake_rx.set_nonblocking(true)?;
        let shared = Shared::new(dispatcher, config);
        let active = Arc::new(AtomicUsize::new(0));
        let accept = {
            let (shared, active) = (shared.clone(), active.clone());
            std::thread::Builder::new()
                .name("flare-ctl-accept".into())
                .spawn(move || accept_loop(&listener, &wake_rx, &shared, &active, uid))?
        };
        Ok(Server {
            shared,
            name: name.into(),
            accept: Some(accept),
            active,
            wake,
            bound,
            _lock: lock,
        })
    }

    /// Queues an event for every client that said hello (and, with `tab`, belongs to that tab).
    /// Never blocks on a client. Returns how many got it.
    pub fn publish(&self, event: &str, params: Value, tab: Option<&str>) -> usize {
        self.shared.hub.publish(event, params, tab)
    }

    /// Queues an event for the clients that said hello with the token of mod or plugin `owner` (its helper).
    pub fn publish_to_owner(&self, event: &str, params: Value, owner: &str) -> usize {
        self.shared.hub.publish_to_owner(event, params, owner)
    }

    /// Clients connected now.
    pub fn client_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// The socket path.
    pub fn pipe(&self) -> &str {
        &self.name
    }

    /// Stops accepting, tells every worker to leave (shutting down sockets whose writes block on a
    /// client that never reads) and waits briefly for them and for in-flight calls. Returns how many
    /// dispatcher calls are still running, so the app can wait on them inside its own quit cap.
    pub fn stop(mut self) -> usize {
        self.shutdown()
    }

    fn shutdown(&mut self) -> usize {
        let Some(accept) = self.accept.take() else {
            return self.shared.in_flight.load(Ordering::SeqCst);
        };
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.hub.abort_all();
        let _ = (&self.wake).write_all(b"x");
        let started = Instant::now();
        while !accept.is_finished() && started.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: quit, bounded by STOP_WAIT
        }
        if accept.is_finished() {
            let _ = accept.join(); // sync-ok: the accept thread has finished
        } else {
            log::warn!("flare-ctl: the accept thread did not stop; leaving it behind");
        }
        // The name goes with the server, but only while it is still the socket this server bound.
        // sync-ok: quit, a local stat and unlink
        if let Ok(meta) = std::fs::symlink_metadata(&self.name)
            && (meta.dev(), meta.ino()) == self.bound
        {
            let _ = std::fs::remove_file(&self.name); // sync-ok: quit, a local unlink
            // The lock file goes too, still under the lock: a start waiting on it re-takes the lock on
            // the file then at the path (see `take_server_lock`).
            let _ = std::fs::remove_file(lock_path(Path::new(&self.name))); // sync-ok: quit, a local unlink
        }
        // A shutdown fails a write blocked now; it repeats until the workers are out.
        while self.active.load(Ordering::SeqCst) > 0 && started.elapsed() < STOP_WAIT {
            self.shared.hub.abort_all();
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: quit, bounded by STOP_WAIT
        }
        while self.shared.in_flight.load(Ordering::SeqCst) > 0 && started.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: quit, bounded by STOP_WAIT
        }
        let left = self.shared.in_flight.load(Ordering::SeqCst);
        if left > 0 {
            log::warn!("flare-ctl: {left} call(s) still running at stop");
        }
        left
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Waits on the listener and the wake socket together; a byte on the wake socket (or the stop flag)
/// ends the loop.
fn accept_loop(
    listener: &UnixListener,
    wake: &UnixStream,
    shared: &Arc<Shared>,
    active: &Arc<AtomicUsize>,
    me: u32,
) {
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let mut fds = [
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two live fds in a local array of the stated length.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) }; // sync-ok: accept thread waits for a client
        if shared.stop.load(Ordering::SeqCst) || fds[1].revents != 0 {
            return;
        }
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                log::warn!("flare-ctl: poll on the control socket failed: {e}");
                std::thread::sleep(Duration::from_millis(100)); // sync-ok: accept thread, retrying
            }
            continue;
        }
        if fds[0].revents == 0 {
            continue;
        }
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::Interrupted {
                    log::warn!("flare-ctl: accept failed: {e}");
                }
                continue;
            }
        };
        // Only this user: the 0600 socket in a 0700 folder already says so; checked again here, before
        // a byte is read or written.
        if !peer_allowed(me, peer_uid(stream.as_raw_fd())) {
            log::warn!("flare-ctl: a client of another user was refused");
            continue;
        }
        // The listener is non-blocking; the connection's own reads and writes are not.
        let _ = stream.set_nonblocking(false);
        admit(PipeEnd::new(stream), shared, active);
    }
}

/// Starts a worker for a connected client, or refuses it when the server is full.
fn admit(end: PipeEnd, shared: &Arc<Shared>, active: &Arc<AtomicUsize>) {
    if active.fetch_add(1, Ordering::SeqCst) >= shared.config.max_clients {
        active.fetch_sub(1, Ordering::SeqCst);
        let e = CtlError::new(
            code::TOO_MANY_CLIENTS,
            "Flare serves too many clients already",
        );
        let _ = write_line(&end, &to_line(&Response::err(Value::Null, e)), MAX_LINE);
        return; // dropping the socket closes it; the client still reads what was written
    }
    let (shared, active2) = (shared.clone(), active.clone());
    let raw = end.0.as_raw_fd();
    let spawned = std::thread::Builder::new()
        .name("flare-ctl-client".into())
        .spawn(move || {
            let handle = ConnHandle::new(NEXT_CONN.fetch_add(1, Ordering::SeqCst));
            shared.hub.add(handle.clone());
            handle.set_abort(Some(Box::new(move || {
                // SAFETY: the hook is cleared (waiting for a running call) before `end` closes the fd,
                // so `raw` is this connection's live socket whenever this runs. A shutdown fails a
                // blocked write at once and ends the peer's reads.
                unsafe { libc::shutdown(raw, libc::SHUT_RDWR) };
            })));
            serve_connection(&end, &shared, &handle);
            handle.set_abort(None);
            shared.hub.remove(handle.id);
            active2.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a client knows of the server before it sends a byte, checked as a pure function: the uid must
/// be this user's, and when the shell was told which process serves the channel, the pid must be it.
pub fn verify_server(
    me: u32,
    server_uid: Option<u32>,
    server_pid: Option<u32>,
    expect_pid: Option<u32>,
) -> Result<(), ClientError> {
    let Some(pid) = server_pid else {
        return Err(ClientError::Protocol(
            "could not tell who serves the pipe".into(),
        ));
    };
    // A shell of a Flare tab is told which process serves the channel (FLARE_CTL_PID). Another process
    // of the same user could have bound the name first, and would then be handed the tab's secret: only
    // the process Flare named is spoken to.
    if expect_pid.is_some_and(|want| want != pid) {
        return Err(ClientError::Protocol(
            "the pipe is not served by the Flare that started this tab".into(),
        ));
    }
    if !peer_allowed(me, server_uid) {
        return Err(ClientError::Protocol(
            "the pipe is served by another user".into(),
        ));
    }
    Ok(())
}

/// Whether a failed connect is a busy server worth waiting for: WouldBlock, or a refusal while the
/// socket is still there and a server holds its lock. A missing socket, or one whose lock is free (a
/// dead Flare's), fails at once.
fn busy_retry(e: &io::Error, socket: &Path) -> bool {
    match e.kind() {
        io::ErrorKind::WouldBlock => true,
        io::ErrorKind::ConnectionRefused => {
            // sync-ok: a client process's connect, a local stat
            std::fs::symlink_metadata(socket).is_ok_and(|m| m.file_type().is_socket())
                && server_alive(socket) == Some(true)
        }
        _ => false,
    }
}

/// Connects as a client and checks the server before a byte is sent.
fn open_verified(
    name: &str,
    timeout: Duration,
    expect_pid: Option<u32>,
    me: u32,
) -> Result<PipeEnd, ClientError> {
    let started = Instant::now();
    let stream = loop {
        // sync-ok: a client process's connect, bounded by its timeout
        match UnixStream::connect(name) {
            Ok(s) => break s,
            // A full backlog (macOS refuses the connect, ECONNREFUSED, as it does for a dead socket):
            // wait for the server to take the queued clients, as for a busy pipe, while it is alive.
            Err(e) if busy_retry(&e, Path::new(name)) => {
                if started.elapsed() >= timeout {
                    return Err(ClientError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(20)); // sync-ok: a client process, bounded
            }
            Err(e) => return Err(ClientError::Io(e)),
        }
    };
    let fd = stream.as_raw_fd();
    verify_server(me, peer_uid(fd), peer_pid(fd), expect_pid)?;
    Ok(PipeEnd::new(stream))
}

/// Connects to the channel `name`. `timeout` bounds waiting for a busy server.
pub fn connect(name: &str, timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    open_verified(name, timeout, None, own_uid()).map(Client::new)
}

/// [`connect`] that also requires the channel to be served by process `pid` (the `FLARE_CTL_PID` a
/// Flare tab's shell is given): a squatter of the same user that bound the name first is refused.
pub fn connect_to_server(
    name: &str,
    timeout: Duration,
    pid: u32,
) -> Result<Client<PipeEnd>, ClientError> {
    open_verified(name, timeout, Some(pid), own_uid()).map(Client::new)
}

/// `connect`, believing this client runs as `uid`: a test seam that makes the real server a foreign
/// user, so the client's refusal is proven on the live socket.
#[doc(hidden)]
pub fn connect_as(name: &str, timeout: Duration, uid: u32) -> Result<Client<PipeEnd>, ClientError> {
    open_verified(name, timeout, None, uid).map(Client::new)
}

/// Connects to the channel from `FLARE_CTL_PIPE` or the user's default.
pub fn connect_default(timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    let name = default_pipe_name()
        .ok_or_else(|| ClientError::Io(io::Error::other("could not work out the pipe name")))?;
    connect(&name, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_rule_wants_a_private_folder_of_this_user() {
        assert!(check_private_dir(true, 501, 0o40700, 501).is_ok());
        assert!(check_private_dir(false, 501, 0o700, 501).is_err());
        assert!(check_private_dir(true, 502, 0o700, 501).is_err());
        for open in [0o710, 0o701, 0o750, 0o755, 0o777, 0o1777] {
            assert!(check_private_dir(true, 501, open, 501).is_err(), "{open:o}");
        }
    }

    #[test]
    fn the_lock_file_sits_beside_the_socket() {
        assert_eq!(
            lock_path(Path::new("/x/flare-ctl/ab.sock")),
            PathBuf::from("/x/flare-ctl/ab.sock.lock")
        );
    }

    #[test]
    fn the_default_socket_path_fits_in_sun_path() {
        let p = pipe_name(
            "4294967295",
            Some("/Users/someone/a very long profile folder name"),
        );
        assert!(p.len() <= MAX_PATH, "{p} is {} bytes", p.len());
        assert!(p.ends_with(".sock"));
    }
}
