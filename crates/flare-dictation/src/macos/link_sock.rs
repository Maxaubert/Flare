//! The dictation link's transport on macOS (port phase 7): a Unix socket with the named pipe's guarantees. The
//! protocol above it (the key, the nonces, the proofs, the caps, the drops) is `link.rs`, shared with Windows.
//!
//! - **Where**: `<per-user temp folder>/flare-dictation/<24 hex>.sock` (`link_pipe`). The temp folder is the one
//!   the system gives this user (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, mode 0700); the folder in it is made mode
//!   0700 and must be a real folder owned by this user with no group or other bits, else the service refuses to
//!   start. The socket itself is mode 0600.
//! - **Who may connect**: on accept the service reads the client's uid (`getpeereid`) and drops any other user
//!   before reading a byte (the Windows DACL's job). The client reads the service's uid the same way before it
//!   sends a byte, and hangs up on another user's socket. The HMAC handshake then proves both hold the key.
//! - **One service per name**: the service takes an exclusive `flock` on `<socket>.lock` and holds it for its
//!   whole life; a second start fails (as `FILE_FLAG_FIRST_PIPE_INSTANCE` does). Only under the lock is a socket
//!   already there removed, as the dead one of a copy that crashed. Closing removes the socket and the lock file
//!   while still holding the lock, and only when the socket is still the one this service bound.
//! - **Cancelling**: `Conn::cancel` shuts the socket down, which ends a read and fails a write that waits on a
//!   peer that never reads (the Windows `CancelIoEx`). Reads and writes wait in `poll` slices of at most
//!   `SLICE_MS`, so a cancel is seen within one slice even if the shutdown were not. `SO_NOSIGPIPE` keeps a write
//!   to a gone peer an error, not a signal.

use std::fs::File;
use std::io;
use std::net::Shutdown;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{lock, start_peer, AcceptHook, LinkService, OnError, Shared, RETRY_MS, WRITE_MS};

/// The longest socket path macOS takes (`sun_path` is 104 bytes with its NUL).
pub const MAX_PATH: usize = 103;
/// The longest one wait in `poll` lasts, so a stop or a cancel is seen this soon.
const SLICE_MS: u64 = 50;
/// How long a start keeps trying for the lock (another start may hold it for an instant).
const LOCK_TRIES: Duration = Duration::from_millis(250);

/// The per-user temp folder as the system names it (`/var/folders/.../T/`, mode 0700), else `$TMPDIR`.
fn user_temp_dir() -> PathBuf {
    let mut buf = vec![0u8; 1024];
    // SAFETY: the buffer is writable for its stated length; confstr writes a NUL-terminated string of at most
    // that many bytes and returns the length it needed.
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

/// The folder that holds this user's link sockets (made by the service when it starts).
pub fn socket_dir() -> PathBuf {
    user_temp_dir().join(flare_win::names::DICTATION_PIPE_PREFIX.trim_end_matches('-'))
}

fn own_uid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { libc::geteuid() }
}

/// The rule both ends apply to the peer's uid: only this user, and a uid that cannot be read is refused.
pub fn peer_allowed(me: u32, peer: Option<u32>) -> bool {
    peer == Some(me)
}

/// The peer's effective uid as the kernel says (`getpeereid`); None when it cannot be read.
fn peer_uid(fd: RawFd) -> Option<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a socket fd and two out-values that outlive the call.
    let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    (r == 0).then_some(uid)
}

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

/// Waits at most `ms` for `events` on `fd`. True when something is ready (or the socket is shut or broken).
fn poll_one(fd: RawFd, events: libc::c_short, ms: u64) -> bool {
    let mut p = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one pollfd that outlives the call.
    let r = unsafe { libc::poll(&mut p, 1, ms.min(i32::MAX as u64) as libc::c_int) };
    r > 0
}

/// The next wait: at most a slice, at most until the deadline. None when the deadline has passed.
fn slice(deadline: Option<Instant>) -> Option<u64> {
    match deadline {
        None => Some(SLICE_MS),
        Some(d) => {
            let left = d.saturating_duration_since(Instant::now());
            if left.is_zero() {
                None
            } else {
                Some((left.as_millis() as u64).clamp(1, SLICE_MS))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The event
// ---------------------------------------------------------------------------------------------

/// A manual-reset event (the Windows one's shape): once set it stays set.
pub(super) struct Event {
    set: Mutex<bool>,
    cv: Condvar,
}

impl Event {
    /// Never fails here; `Option` for the Windows shape.
    pub(super) fn new() -> Option<Event> {
        Some(Event {
            set: Mutex::new(false),
            cv: Condvar::new(),
        })
    }
    pub(super) fn none() -> Event {
        Event {
            set: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
    pub(super) fn is_real(&self) -> bool {
        true
    }
    pub(super) fn set(&self) {
        *lock(&self.set) = true;
        self.cv.notify_all();
    }
    /// Waits for the event, at most `ms` (`None` is forever); true when it was set.
    pub(super) fn wait(&self, ms: Option<u64>) -> bool {
        let g = lock(&self.set);
        match ms {
            Some(0) => *g,
            Some(ms) => {
                let (g, _) = self
                    .cv
                    .wait_timeout_while(g, Duration::from_millis(ms), |s| !*s)
                    .unwrap_or_else(|e| e.into_inner());
                *g
            }
            None => *self
                .cv
                .wait_while(g, |s| !*s)
                .unwrap_or_else(|e| e.into_inner()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A connection
// ---------------------------------------------------------------------------------------------

/// One connected socket end: a read and a write may wait at once, from different threads, and `cancel` ends
/// whatever is waiting.
pub(super) struct Conn {
    stream: UnixStream,
    cancelled: AtomicBool,
    write_lock: Mutex<()>,
}

impl Conn {
    pub(super) fn new(stream: UnixStream) -> Option<Arc<Conn>> {
        // Blocking mode is never relied on: every call below is MSG_DONTWAIT behind a poll.
        no_sigpipe(stream.as_raw_fd());
        Some(Arc::new(Conn {
            stream,
            cancelled: AtomicBool::new(false),
            write_lock: Mutex::new(()),
        }))
    }

    fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    pub(super) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Reads some bytes; `Ok(0)` is the end of the stream. Ends with an error at the deadline or on a cancel.
    pub(super) fn read(&self, buf: &mut [u8], deadline: Option<Instant>) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            // SAFETY: a live socket fd and a writable buffer of `buf.len()` bytes; MSG_DONTWAIT makes this one
            // call non-blocking.
            let n = unsafe {
                libc::recv(
                    self.fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => {}
                io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::NotConnected => return Ok(0),
                _ => return Err(e),
            }
            let Some(ms) = slice(deadline) else {
                return Err(io::ErrorKind::TimedOut.into());
            };
            poll_one(self.fd(), libc::POLLIN, ms);
        }
    }

    /// Writes every byte, one writer at a time, giving up after `WRITE_MS` or when cancelled.
    pub(super) fn write(&self, data: &[u8]) -> io::Result<()> {
        let _one = lock(&self.write_lock);
        let deadline = Some(Instant::now() + Duration::from_millis(WRITE_MS));
        let mut done = 0usize;
        while done < data.len() {
            if self.cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let rest = &data[done..];
            // SAFETY: a live socket fd and `rest.len()` readable bytes; MSG_DONTWAIT never parks the thread.
            let n = unsafe {
                libc::send(
                    self.fd(),
                    rest.as_ptr().cast(),
                    rest.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n > 0 {
                done += n as usize;
                continue;
            }
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => {}
                _ => return Err(e),
            }
            let Some(ms) = slice(deadline) else {
                return Err(io::ErrorKind::TimedOut.into());
            };
            poll_one(self.fd(), libc::POLLOUT, ms);
        }
        Ok(())
    }
}

/// Opens the client end; `None` when there is no service now, or the socket is served by another user.
pub(super) fn connect(pipe: &str) -> Option<Arc<Conn>> {
    // sync-ok: the link client's own thread; a local connect that never waits on the peer
    let stream = UnixStream::connect(pipe).ok()?;
    // Another user's socket is never spoken to, not even the hello (the impostor learns nothing).
    if !peer_allowed(own_uid(), peer_uid(stream.as_raw_fd())) {
        return None;
    }
    Conn::new(stream)
}

// ---------------------------------------------------------------------------------------------
// The folder, the lock and the listener
// ---------------------------------------------------------------------------------------------

/// The folder rule as a pure function: a folder, owned by `me`, no group or other permission bits.
pub(super) fn check_private_dir(
    is_dir: bool,
    owner: u32,
    mode: u32,
    me: u32,
) -> Result<(), &'static str> {
    if !is_dir {
        return Err("the link's folder is not a folder");
    }
    if owner != me {
        return Err("the link's folder belongs to another user");
    }
    if mode & 0o077 != 0 {
        return Err("the link's folder is open to other users");
    }
    Ok(())
}

/// Makes `dir` (mode 0700) when it is missing and checks it is this user's alone.
fn ensure_private_dir(dir: &Path, me: u32) -> Result<(), String> {
    // sync-ok: the link's acceptor start, a local mkdir
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    // sync-ok: the link's start, a local stat
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if meta.is_dir() && meta.uid() == me && meta.mode() & 0o077 != 0 {
        // This user's own folder, opened up (copied, restored, chmodded): made private again (rule 319). Through a
        // descriptor opened without following a link, so a folder swapped for a symlink meanwhile is never touched.
        tighten_own_dir(dir, me);
    }
    // sync-ok: the link's start, a local stat
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    check_private_dir(meta.is_dir(), meta.uid(), meta.mode(), me)
        .map_err(|why| format!("{}: {why}", dir.display()))
}

/// `chmod 0700` on `dir` when it is a real folder (never a link) of `me`; the caller checks again after.
fn tighten_own_dir(dir: &Path, me: u32) {
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)
    // sync-ok: the link's start, a local open
    else {
        return;
    };
    let Ok(m) = f.metadata() else { return };
    if m.is_dir() && m.uid() == me {
        // SAFETY: fchmod on a descriptor this function owns.
        if unsafe { libc::fchmod(f.as_raw_fd(), 0o700) } == 0 {
            log::debug!(
                "dictation link: the link's folder was open to others and is private again"
            );
        }
    }
}

fn lock_path(socket: &Path) -> PathBuf {
    let mut p = socket.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

/// `flock` without waiting: Ok(true) when taken, Ok(false) when another open file holds it.
fn try_flock(file: &File, op: libc::c_int) -> io::Result<bool> {
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

/// Takes the service's lock for `socket`, or fails while another service holds it. A lock taken on a file that
/// was unlinked meanwhile (a service that closed) is let go and taken again on the file now at the path, so two
/// services never each hold a lock of their own.
fn take_lock(socket: &Path) -> Result<File, String> {
    let path = lock_path(socket);
    let started = Instant::now();
    loop {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&path) // sync-ok: the link's start, a local open
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.uid() != own_uid() {
            return Err(format!("{} is not this user's lock file", path.display()));
        }
        if try_flock(&file, libc::LOCK_EX).map_err(|e| e.to_string())? {
            // sync-ok: the link's start, a local stat
            let now = std::fs::symlink_metadata(&path).ok();
            if now.is_some_and(|m| (m.dev(), m.ino()) == (meta.dev(), meta.ino())) {
                return Ok(file);
            }
            continue;
        }
        if started.elapsed() >= LOCK_TRIES {
            return Err("another copy already serves the dictation link".into());
        }
        std::thread::sleep(Duration::from_millis(10)); // sync-ok: the link's start, bounded by LOCK_TRIES
    }
}

/// The bound socket, the lock that makes it this copy's, and what it was bound as (to remove only that one).
pub(super) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    id: (u64, u64),
    lock: Option<File>,
}

/// Binds `path` for this user only. A socket already there is the dead one of a copy that did not quit cleanly
/// (no live service holds the lock) and is removed first; anything else in the way is an error.
pub(super) fn bind(path: &Path) -> Result<Listener, String> {
    if path.as_os_str().len() > MAX_PATH {
        return Err(format!(
            "the link's socket path is too long: {}",
            path.display()
        ));
    }
    let dir = path.parent().ok_or("the link's socket has no folder")?;
    let me = own_uid();
    ensure_private_dir(dir, me)?;
    let lock = take_lock(path)?;
    // sync-ok: the link's start, a local stat
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(format!(
                "{} is in the way of the link's socket",
                path.display()
            ));
        }
        // A process that serves the name without the lock (not a Flare) is never replaced.
        // sync-ok: the link's start, a local connect that never waits on the peer
        if UnixStream::connect(path).is_ok() {
            return Err("another process already serves the dictation link".into());
        }
        // sync-ok: the link's start, removes the dead socket of a copy that crashed
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    // sync-ok: the link's start, a local chmod
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?; // sync-ok: a local stat
    Ok(Listener {
        listener,
        path: path.to_path_buf(),
        id: (meta.dev(), meta.ino()),
        lock: Some(lock),
    })
}

pub(super) enum Accepted {
    Connected(UnixStream),
    Stopped,
    Failed(String),
}

impl Listener {
    /// Waits for a client of this user, or for `stop`. Another user's connection is closed before a byte of it
    /// is read, and the wait goes on.
    pub(super) fn accept(&self, stop: &Event) -> Accepted {
        loop {
            if stop.wait(Some(0)) {
                return Accepted::Stopped;
            }
            match self.listener.accept() {
                Ok((s, _)) => {
                    if !peer_allowed(own_uid(), peer_uid(s.as_raw_fd())) {
                        drop(s);
                        continue;
                    }
                    if s.set_nonblocking(false).is_err() {
                        continue;
                    }
                    return Accepted::Connected(s);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    poll_one(self.listener.as_raw_fd(), libc::POLLIN, SLICE_MS);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // A client that left between being queued and accepted is not the listener's failure.
                Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(e) => return Accepted::Failed(format!("the link could not accept ({e})")),
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Still under the lock: remove the socket only while it is the one this service bound, then the lock
        // file (a start waiting on it sees it unlinked and takes the new one).
        if let Ok(m) = std::fs::symlink_metadata(&self.path) {
            if (m.dev(), m.ino()) == self.id {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        if let Some(l) = self.lock.take() {
            let _ = std::fs::remove_file(lock_path(&self.path));
            drop(l);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The service's start
// ---------------------------------------------------------------------------------------------

/// Binds the socket and starts the acceptor; inert (and `on_error` told why) when the name is taken or the
/// folder is not this user's alone.
pub(super) fn start_service(
    pipe: &str,
    key: [u8; 32],
    on_error: OnError,
    proof_ms: u64,
    hook: Option<AcceptHook>,
) -> LinkService {
    let shared = Shared::new(key, Event::none(), proof_ms);
    let listener = match bind(Path::new(pipe)) {
        Ok(l) => l,
        Err(e) => {
            on_error(e);
            return LinkService::inert(shared);
        }
    };
    let sh = Arc::clone(&shared);
    let spawned = std::thread::Builder::new()
        .name("flare-link-accept".into())
        .spawn(move || acceptor(&sh, &listener, &on_error, hook.as_ref()));
    LinkService {
        shared,
        acceptor: Mutex::new(spawned.ok()),
    }
}

fn acceptor(
    shared: &Arc<Shared>,
    listener: &Listener,
    on_error: &OnError,
    hook: Option<&AcceptHook>,
) {
    loop {
        match listener.accept(&shared.stop) {
            Accepted::Connected(stream) => {
                if let Some(h) = hook {
                    h();
                }
                if let Some(conn) = Conn::new(stream) {
                    start_peer(shared, conn);
                }
            }
            Accepted::Stopped => return,
            Accepted::Failed(why) => {
                on_error(why);
                if shared.stop.wait(Some(RETRY_MS)) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_user_may_be_the_peer_and_an_unreadable_uid_is_refused() {
        assert!(peer_allowed(501, Some(501)));
        assert!(!peer_allowed(501, Some(0)));
        assert!(!peer_allowed(501, Some(502)));
        assert!(!peer_allowed(501, None));
    }

    #[test]
    fn the_folder_must_be_a_folder_of_this_user_with_no_group_or_other_bits() {
        assert!(check_private_dir(true, 501, 0o40700, 501).is_ok());
        assert!(check_private_dir(false, 501, 0o700, 501).is_err());
        assert!(check_private_dir(true, 0, 0o700, 501).is_err());
        assert!(check_private_dir(true, 501, 0o750, 501).is_err());
        assert!(check_private_dir(true, 501, 0o701, 501).is_err());
    }

    #[test]
    fn this_users_own_folder_left_open_is_made_private_and_a_symlink_is_still_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("flare-dictation");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_dir(&dir, own_uid()).is_ok());
        let mode = std::fs::symlink_metadata(&dir).unwrap().mode();
        assert_eq!(mode & 0o777, 0o700, "made private again");
        // A symlink to an open folder of this user: refused, and the target left as it was.
        let target = tmp.path().join("open");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_private_dir(&link, own_uid()).is_err());
        assert_eq!(
            std::fs::symlink_metadata(&target).unwrap().mode() & 0o777,
            0o755
        );
        // Another user's folder is never changed (here: "another user" is any uid but ours).
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(ensure_private_dir(&dir, own_uid().wrapping_add(1)).is_err());
        assert_eq!(
            std::fs::symlink_metadata(&dir).unwrap().mode() & 0o777,
            0o750
        );
    }

    #[test]
    fn the_peer_of_a_socket_pair_is_this_user() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(a.as_raw_fd()), Some(own_uid()));
    }

    #[test]
    fn the_socket_folder_is_in_the_per_user_temp_folder_and_short_enough() {
        let dir = socket_dir();
        assert!(dir.ends_with("flare-dictation"), "{}", dir.display());
        // The longest name the link makes: 24 hex and `.sock`.
        let longest = dir.join(format!("{}.sock", "f".repeat(24)));
        assert!(
            longest.as_os_str().len() <= MAX_PATH,
            "{} is longer than a socket path may be",
            longest.display()
        );
    }

    #[test]
    fn a_cancel_ends_a_read_that_waits_forever_and_a_write_to_a_peer_that_never_reads() {
        let (a, _b) = UnixStream::pair().unwrap();
        let conn = Conn::new(a).unwrap();
        let c = Arc::clone(&conn);
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 16];
            c.read(&mut buf, None)
        });
        std::thread::sleep(Duration::from_millis(100));
        conn.cancel();
        let r = reader.join().unwrap();
        assert!(r.is_err() || r.as_ref().ok() == Some(&0), "{r:?}");

        // A peer that never reads: the write waits for room, then the cancel ends it.
        let (a, _b) = UnixStream::pair().unwrap();
        let conn = Conn::new(a).unwrap();
        let c = Arc::clone(&conn);
        let writer = std::thread::spawn(move || c.write(&vec![b'x'; 8 * 1024 * 1024]));
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        conn.cancel();
        assert!(writer.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_read_with_a_deadline_times_out() {
        let (a, _b) = UnixStream::pair().unwrap();
        let conn = Conn::new(a).unwrap();
        let mut buf = [0u8; 4];
        let started = Instant::now();
        let r = conn.read(&mut buf, Some(Instant::now() + Duration::from_millis(150)));
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(140));
    }

    #[test]
    fn the_event_stays_set_and_a_wait_ends_when_it_is_set() {
        let ev = Arc::new(Event::none());
        assert!(!ev.wait(Some(0)));
        assert!(!ev.wait(Some(20)));
        let e = Arc::clone(&ev);
        let t = std::thread::spawn(move || e.wait(None));
        std::thread::sleep(Duration::from_millis(50));
        ev.set();
        assert!(t.join().unwrap());
        assert!(ev.wait(Some(0)));
    }
}
