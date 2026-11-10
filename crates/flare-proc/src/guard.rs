//! macOS: a program started in a group and session of its own, with a guard that ends the whole tree
//! when Flare lets it go, when the program itself ends, or when Flare dies, a crash included. This is
//! what a kill-on-close job object gives a plugin helper on Windows (regression rule 1053).
//!
//! - **The group.** Between fork and exec the child calls `setsid`: it leads a new session and a new
//!   process group with its own pid as both ids, and has no controlling terminal. Everything it starts
//!   inherits both, unless it leaves them.
//! - **The guard.** Before its exec the child forks twice: the guard is a grandchild that its middle
//!   process leaves an orphan (launchd reaps it), so the program never sees an unknown child. The guard
//!   at once execs the guard entry ([`use_this_program`]: Flare's own executable, whose `main` calls
//!   [`run_if_guard`] first), so it holds none of Flare's memory (a fork that never execs would keep a
//!   copy-on-write snapshot of all of it, rule 1054). Its `argv[0]` is [`MARK`] with the descriptors it
//!   keeps and Flare's pid. As the guard it leads a group of its own inside the program's session (so a
//!   helper that kills its own group, `kill -9 0`, misses it), ignores the polite signals (HUP, INT,
//!   TERM, QUIT), closes every descriptor but one end of a socket pair whose other end only Flare holds,
//!   tells Flare its pid on it, lets the program go on (the program waits in its start until then), and
//!   waits in kqueue for any of three ends: the socket's EOF (Flare dropped its [`Guard`], or Flare's
//!   process is gone, a crash included, since the kernel closes a dead process's descriptors),
//!   `NOTE_EXIT` of the program, and `NOTE_EXIT` of Flare's pid.
//! - **Escapees.** Every [`TICK`] the guard notes the program's descendants (libproc child lists, from
//!   the program down, with each one's start time so a recycled pid is never taken for one). A process
//!   that left the group (`setpgid`) is still in the session; one that left the session too (`setsid`)
//!   is still a descendant the guard has seen. At the end the guard sends `SIGKILL` to every process of
//!   the group, of the session and of its list, again until a pass finds none, and then exits.
//! - **What runs before the exec.** The program and the middle process are forks of a threaded process,
//!   so between fork and exec they call only system calls on memory set aside before the fork (no
//!   allocation, no lock): `setsid`, `pipe`, `fork`, `waitpid`, `read`, `open`, `dup2`, `close`,
//!   `fcntl`, `execve` and `_exit`.
//!
//! What it cannot catch: a process that left the session and was left an orphan by its parent within
//! one [`TICK`] (a double-forking daemon), if no scan saw it before its parent ended; and, when the
//! helper finds and kills the guard itself by its pid, a process that left the session (Flare's own
//! pass, [`end_session`], still ends the session and the group).

use std::ffi::{CString, c_char};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How often the guard notes the program's descendants.
pub const TICK: Duration = Duration::from_millis(250);
/// The `argv[0]` of a guard: `flare-guard:<socket fd>:<status fd>:<Flare's pid>`.
pub const MARK: &str = "flare-guard:";
/// The error a start fails with when its guard could not start (no `execve` fails with it).
pub const NO_GUARD: i32 = libc::ECHILD;
/// The most descendants the guard keeps in its list (the group and the session catch the rest).
const TRACK_MAX: usize = 512;
/// Room for this many pids in the guard's scan of the whole table.
const PIDS_MAX: usize = 16 * 1024;
/// The guard's final passes: at most this many, a moment apart.
const KILL_PASSES: usize = 64;
/// Descriptors above this are not closed by the guard (none of Flare's are that high).
const FD_CLOSE_MAX: libc::c_int = 1 << 16;
/// The most arguments an entry adds after `argv[0]`.
const EXTRA_MAX: usize = 6;

/// The program a guard execs, with the arguments after its `argv[0]`.
struct Entry {
    exe: CString,
    extra: Vec<CString>,
}

static ENTRY: OnceLock<Entry> = OnceLock::new();

/// Flare's `main`: guards are this executable, which [`run_if_guard`] turns into a guard. Until an
/// entry is set, [`guard_command`] refuses. The first entry set stays.
pub fn use_this_program() -> io::Result<()> {
    set_entry(&std::env::current_exe()?, &[])
}

/// A test binary: guards are this binary running only the test `test` (its full path, `--exact`), a
/// `#[test]` whose body is [`run_if_guard`]. Never the whole binary: that would run every test.
pub fn use_test_entry(test: &str) -> io::Result<()> {
    set_entry(&std::env::current_exe()?, &[test, "--exact", "--nocapture"])
}

fn set_entry(exe: &Path, extra: &[&str]) -> io::Result<()> {
    let bad = |_| io::Error::other("a guard entry holds a NUL");
    let exe = CString::new(exe.as_os_str().as_bytes()).map_err(bad)?;
    let extra = extra
        .iter()
        .take(EXTRA_MAX)
        .map(|s| CString::new(*s).map_err(bad))
        .collect::<io::Result<Vec<_>>>()?;
    let _ = ENTRY.set(Entry { exe, extra });
    Ok(())
}

/// Flare's end of a guarded program: one end of the socket pair. Dropping it, or [`Guard::release`],
/// tells the guard to end the whole tree; so does Flare's exit, however it happens.
#[derive(Debug)]
pub struct Guard {
    sock: Option<OwnedFd>,
    id: Option<GuardId>,
}

impl Guard {
    /// Lets the program's tree go: the guard ends every process of it (a moment later, not here).
    pub fn release(&mut self) {
        self.sock = None;
    }

    /// The guard process (None when it was gone before Flare read its pid).
    pub fn id(&self) -> Option<GuardId> {
        self.id
    }
}

/// One guard process: its pid and when it started, so a recycled pid is never taken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuardId {
    pub pid: u32,
    start_us: u64,
}

impl GuardId {
    fn of(pid: u32) -> Option<Self> {
        let start_us = crate::stat(pid)?.start_us?;
        Some(Self { pid, start_us })
    }

    /// True while the guard runs (not a zombie, and its pid still names it).
    pub fn alive(&self) -> bool {
        crate::stat(self.pid).and_then(|s| s.start_us) == Some(self.start_us)
    }

    /// BLOCKING, at most `max`: waits until the guard has ended. True when it has.
    pub fn wait_gone(&self, max: Duration) -> bool {
        let started = Instant::now();
        while self.alive() {
            if started.elapsed() >= max {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: the documented blocking wait
        }
        true
    }
}

/// A [`Guard`] being set up on a [`Command`] that is not started yet.
#[derive(Debug)]
pub struct Pending {
    mine: OwnedFd,
    theirs: OwnedFd,
}

impl Pending {
    /// The command was started (so its guard runs and has said its pid): Flare's copy of the guard's
    /// end is closed, its own end is kept.
    pub fn started(self) -> Guard {
        drop(self.theirs);
        let mut b = [0u8; 4];
        // SAFETY: a live descriptor and a buffer of the length passed; no wait.
        let n = unsafe {
            libc::recv(
                self.mine.as_raw_fd(),
                b.as_mut_ptr().cast(),
                b.len(),
                libc::MSG_DONTWAIT,
            )
        };
        let id = (n == 4)
            .then(|| u32::try_from(i32::from_ne_bytes(b)).ok())
            .flatten()
            .and_then(GuardId::of);
        Guard {
            sock: Some(self.mine),
            id,
        }
    }
}

/// A Unix socket pair whose two ends are close-on-exec, so no program Flare starts later inherits
/// them. `socketpair` and `fcntl` are two calls: a process another thread starts in between keeps a
/// copy (macOS has no atomic form). That only delays the guard's EOF; a stop also kills the helper,
/// whose end the guard sees, and Flare's own end is watched by its pid (rule 1053).
fn socket_pair_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: room for the two descriptors the call writes.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both are fresh descriptors this call owns from here, closed once by the OwnedFds.
    let (a, b) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&a, &b] {
        // SAFETY: a live descriptor and plain values.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((a, b))
}

/// What the guard's `execve` needs, made before the fork. The pointers point into the CStrings it owns.
struct ExecArgs {
    exe: CString,
    extra: Vec<CString>,
    _env: Vec<CString>,
    env_ptrs: Vec<*const c_char>,
}

// SAFETY: the pointers point into the CStrings this struct owns and are only read, in the child.
unsafe impl Send for ExecArgs {}
// SAFETY: as above; nothing is shared mutably.
unsafe impl Sync for ExecArgs {}

/// Writes `n` (not negative) in decimal at `at`; returns the index after it.
fn put_num(buf: &mut [u8], mut at: usize, n: i32) -> usize {
    let mut digits = [0u8; 12];
    let mut len = 0;
    let mut v = n.max(0) as u32;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        buf[at] = digits[len];
        at += 1;
    }
    at
}

/// Pure, no allocation: the NUL-terminated `argv[0]` of a guard, [`MARK`]`<sock>:<status>:<flare>`.
fn write_mark(buf: &mut [u8; 64], sock: i32, status: i32, flare: i32) {
    let m = MARK.as_bytes();
    buf[..m.len()].copy_from_slice(m);
    let mut at = put_num(buf, m.len(), sock);
    buf[at] = b':';
    at = put_num(buf, at + 1, status);
    buf[at] = b':';
    at = put_num(buf, at + 1, flare);
    buf[at] = 0;
}

/// Pure: the descriptors and Flare's pid in what follows [`MARK`]; None unless it is exactly three
/// numbers, the descriptors above 2 and the pid above 1.
fn parse_mark(rest: &[u8]) -> Option<(RawFd, RawFd, libc::pid_t)> {
    let text = std::str::from_utf8(rest).ok()?;
    let mut parts = text.split(':');
    let mut num = || -> Option<i32> {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let (sock, status, flare) = (num()?, num()?, num()?);
    if parts.next().is_some() || sock <= 2 || status <= 2 || sock == status || flare <= 1 {
        return None;
    }
    Some((sock, status, flare))
}

/// Makes `cmd` start in a new session and group of its own with a guard ([`Guard`]). Call
/// [`Pending::started`] once `cmd.spawn()` succeeded; drop the [`Pending`] if it failed. The
/// command's own `pre_exec` and `process_group` must not be set. A start whose guard could not start
/// fails with the raw OS error [`NO_GUARD`]; with no entry set ([`use_this_program`]) this refuses.
pub fn guard_command(cmd: &mut Command) -> io::Result<Pending> {
    let entry = ENTRY
        .get()
        .ok_or_else(|| io::Error::other("no guard program is set"))?;
    let (mine, theirs) = socket_pair_cloexec()?;
    let their_fd = theirs.as_raw_fd();
    let flare = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    // Everything the forks use is made here, before the fork. The guard gets Flare's environment.
    let env: Vec<CString> = std::env::vars_os()
        .filter_map(|(k, v)| {
            let mut kv = k.as_bytes().to_vec();
            kv.push(b'=');
            kv.extend_from_slice(v.as_bytes());
            CString::new(kv).ok()
        })
        .collect();
    let mut env_ptrs: Vec<*const c_char> = env.iter().map(|e| e.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());
    let args = ExecArgs {
        exe: entry.exe.clone(),
        extra: entry.extra.clone(),
        _env: env,
        env_ptrs,
    };
    let mut mark = [0u8; 64];
    // SAFETY: the closure runs between fork and exec and calls only async-signal-safe system calls
    // (see the module comment) on memory made before the fork.
    unsafe {
        cmd.pre_exec(move || {
            // The whole struct is captured (it is Send and Sync), not its fields one by one.
            let args: &ExecArgs = &args;
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            // The guard says on this pipe that it runs; the program waits for that.
            let mut st = [0 as libc::c_int; 2];
            if libc::pipe(st.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            match libc::fork() {
                -1 => {
                    libc::close(st[0]);
                    libc::close(st[1]);
                    Err(io::Error::last_os_error())
                }
                0 => {
                    // The middle process: start the guard, and end at once so it is an orphan.
                    match libc::fork() {
                        0 => {
                            libc::close(st[0]);
                            // Only the guard keeps these two across its exec.
                            if libc::fcntl(their_fd, libc::F_SETFD, 0) == -1
                                || libc::fcntl(st[1], libc::F_SETFD, 0) == -1
                            {
                                libc::_exit(1);
                            }
                            // Not the program's output: nothing the guard's start prints (a test
                            // harness's banner, a loader's warning) reaches the helper's log.
                            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
                            if null < 0 {
                                libc::_exit(1);
                            }
                            for fd in 0..3 {
                                libc::dup2(null, fd);
                            }
                            if null > 2 {
                                libc::close(null);
                            }
                            write_mark(&mut mark, their_fd, st[1], flare);
                            let mut argv: [*const c_char; EXTRA_MAX + 2] =
                                [std::ptr::null(); EXTRA_MAX + 2];
                            argv[0] = mark.as_ptr().cast();
                            for (i, a) in args.extra.iter().enumerate() {
                                argv[i + 1] = a.as_ptr();
                            }
                            libc::execve(args.exe.as_ptr(), argv.as_ptr(), args.env_ptrs.as_ptr());
                            libc::_exit(127)
                        }
                        -1 => libc::_exit(1),
                        _ => libc::_exit(0),
                    }
                }
                middle => {
                    libc::close(st[1]);
                    let mut status = 0;
                    while libc::waitpid(middle, &mut status, 0) == -1
                        && *libc::__error() == libc::EINTR
                    {}
                    let mut b = 0u8;
                    let n = loop {
                        let n = libc::read(st[0], (&raw mut b).cast(), 1);
                        if n == -1 && *libc::__error() == libc::EINTR {
                            continue;
                        }
                        break n;
                    };
                    libc::close(st[0]);
                    // No guard, no start: a program Flare could not end must not run.
                    if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 || n != 1 {
                        return Err(io::Error::from_raw_os_error(NO_GUARD));
                    }
                    Ok(())
                }
            }
        });
    }
    Ok(Pending { mine, theirs })
}

/// The first thing a program that is a guard entry runs ([`use_this_program`], [`use_test_entry`]):
/// when this process was started as a guard (its `argv[0]` is [`MARK`]...), it is the guard and never
/// returns; otherwise it returns at once.
pub fn run_if_guard() {
    let Some(arg0) = std::env::args_os().next() else {
        return;
    };
    let Some(rest) = arg0.as_bytes().strip_prefix(MARK.as_bytes()) else {
        return;
    };
    let code = guard_main(rest);
    // SAFETY: ends this process; nothing of it is to run on.
    unsafe { libc::_exit(code) }
}

/// True when `fd` is open and of file type `kind` (`S_IFSOCK`, `S_IFIFO`).
fn is_kind(fd: RawFd, kind: libc::mode_t) -> bool {
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a live struct; a bad descriptor fails.
    (unsafe { libc::fstat(fd, &mut st) } == 0) && st.st_mode & libc::S_IFMT == kind
}

/// The guard. Returns the exit code when it refuses (a stray start: nothing is signalled), 0 after it
/// ended the tree.
fn guard_main(rest: &[u8]) -> i32 {
    let Some((sock, status, flare)) = parse_mark(rest) else {
        return 2;
    };
    // SAFETY: system calls on plain values.
    unsafe {
        for sig in [
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGQUIT,
            libc::SIGPIPE,
        ] {
            libc::signal(sig, libc::SIG_IGN);
        }
        // Not the plugin folder: the guard holds no folder open.
        libc::chdir(c"/".as_ptr());
        let top = libc::getdtablesize().min(FD_CLOSE_MAX);
        for fd in 0..top {
            if fd != sock && fd != status {
                libc::close(fd);
            }
        }
    }
    if !is_kind(sock, libc::S_IFSOCK) || !is_kind(status, libc::S_IFIFO) {
        return 2;
    }
    // SAFETY: no arguments.
    let (me, program) = unsafe { (libc::getpid(), libc::getsid(0)) };
    if program <= 1 || program == me {
        return 2;
    }
    // A real guard's program is Flare's child, waiting in its start for this guard.
    let program_u = u32::try_from(program).unwrap_or(0);
    if crate::stat(program_u).map(|s| s.ppid) != u32::try_from(flare).ok() {
        return 2;
    }
    // A group of its own in the program's session: `kill 0` from the helper misses the guard.
    // SAFETY: plain values.
    if unsafe { libc::setpgid(0, 0) } != 0 {
        return 2;
    }
    let pid = me.to_ne_bytes();
    // SAFETY: live descriptors and buffers of the lengths passed.
    unsafe {
        if libc::write(sock, pid.as_ptr().cast(), pid.len()) != 4
            || libc::write(status, b"1".as_ptr().cast(), 1) != 1
        {
            return 2;
        }
        libc::close(status);
    }
    let mut mem = Memory::new();
    if let Some(start) = start_of(program) {
        mem.seen.push(Seen {
            pid: program,
            start,
        });
    }
    watch(sock, flare, program, me, &mut mem);
    0
}

/// One descendant the guard has seen: its pid and when it started, so a recycled pid is not it.
#[derive(Clone, Copy, Default)]
struct Seen {
    pid: libc::pid_t,
    start: (u64, u64),
}

/// The guard's buffers.
struct Memory {
    pids: Vec<libc::pid_t>,
    kids: Vec<libc::pid_t>,
    seen: Vec<Seen>,
}

impl Memory {
    fn new() -> Self {
        Self {
            pids: vec![0; PIDS_MAX],
            kids: vec![0; 1024],
            seen: Vec::with_capacity(TRACK_MAX),
        }
    }
}

/// When `pid` started and whether it has ended (a zombie); None when it cannot be read (gone, or
/// another user's).
fn start_of(pid: libc::pid_t) -> Option<(u64, u64)> {
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a live struct of `size` bytes, the size this flavor fills.
    let got =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if got != size || info.pbi_status == libc::SZOMB {
        return None;
    }
    Some((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

/// The guard's watch, then its end of the tree.
fn watch(sock: RawFd, flare: libc::pid_t, program: libc::pid_t, me: libc::pid_t, mem: &mut Memory) {
    // SAFETY: every call below is a system call on plain values or on memory owned by `mem` or the
    // stack.
    unsafe {
        let kq = libc::kqueue();
        let mut end = kq < 0;
        if !end {
            let ev = |ident: usize, filter: i16, fflags: u32| libc::kevent {
                ident,
                filter,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let changes = [
                ev(sock as usize, libc::EVFILT_READ, 0),
                ev(program as usize, libc::EVFILT_PROC, libc::NOTE_EXIT),
                ev(flare as usize, libc::EVFILT_PROC, libc::NOTE_EXIT),
            ];
            for c in &changes {
                // One at a time: a pid that has already ended fails with ESRCH, and that is an end.
                if libc::kevent(kq, c, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
                    end = true;
                }
            }
            // Flare may have ended before its watch was set.
            if libc::kill(flare, 0) != 0 {
                end = true;
            }
        }
        let tick = libc::timespec {
            tv_sec: 0,
            tv_nsec: TICK.as_nanos() as libc::c_long,
        };
        while !end {
            note_descendants(mem);
            let mut out: libc::kevent = std::mem::zeroed();
            let n = libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, &tick);
            if n > 0 || (n < 0 && *libc::__error() != libc::EINTR) {
                end = true;
            }
        }
        let pause = libc::timespec {
            tv_sec: 0,
            tv_nsec: 5_000_000,
        };
        for _ in 0..KILL_PASSES {
            note_descendants(mem);
            if kill_pass(mem, program, me, flare) == 0 {
                break;
            }
            libc::nanosleep(&pause, std::ptr::null_mut());
        }
    }
}

/// Adds the children of every descendant in the list to it, until nothing new is found; drops the ones
/// that have ended (or whose pid now names another process).
fn note_descendants(mem: &mut Memory) {
    mem.seen
        .retain(|s| start_of(s.pid).is_some_and(|now| now == s.start));
    let mut i = 0;
    while i < mem.seen.len() {
        let parent = mem.seen[i];
        let bytes = (mem.kids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `kids` holds `bytes` bytes; the call writes at most that many and returns the count.
        let n =
            unsafe { libc::proc_listchildpids(parent.pid, mem.kids.as_mut_ptr().cast(), bytes) };
        let n = usize::try_from(n).unwrap_or(0).min(mem.kids.len());
        for k in 0..n {
            let kid = mem.kids[k];
            if kid <= 1 || mem.seen.iter().any(|s| s.pid == kid) {
                continue;
            }
            // A child starts after its parent; anything else is a recycled pid.
            if let Some(start) = start_of(kid).filter(|s| *s >= parent.start)
                && mem.seen.len() < mem.seen.capacity()
            {
                mem.seen.push(Seen { pid: kid, start });
            }
        }
        i += 1;
    }
}

/// Sends `SIGKILL` to every live process of the group or session `program` leads and to every listed
/// descendant, never to the guard itself, to Flare or to launchd. Returns how many it sent to.
fn kill_pass(mem: &mut Memory, program: libc::pid_t, me: libc::pid_t, flare: libc::pid_t) -> usize {
    let bytes = (mem.pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: `pids` holds `bytes` bytes; the call writes at most that many and returns the count.
    let n = unsafe { libc::proc_listallpids(mem.pids.as_mut_ptr().cast(), bytes) };
    let n = usize::try_from(n).unwrap_or(0).min(mem.pids.len());
    let mut sent = 0;
    for k in 0..n {
        let pid = mem.pids[k];
        if pid <= 1 || pid == me || pid == flare {
            continue;
        }
        // SAFETY: plain values in.
        let ours = unsafe { libc::getpgid(pid) == program || libc::getsid(pid) == program }
            || mem
                .seen
                .iter()
                .any(|s| s.pid == pid && start_of(pid) == Some(s.start));
        // A zombie has ended already: its parent (or launchd) reaps it.
        if ours && start_of(pid).is_some() {
            // SAFETY: plain values in.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                sent += 1;
            }
        }
    }
    sent
}

/// True while any process (a zombie included) is in group `pgid`.
pub fn group_alive(pgid: u32) -> bool {
    let Some(g) = libc::pid_t::try_from(pgid).ok().filter(|g| *g > 1) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; killpg only checks that the group exists.
    let ok = unsafe { libc::killpg(g, 0) == 0 };
    ok || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// BLOCKING, at most `max`: waits until group `pgid` is empty (its guard is one of its members, so
/// this also waits for the guard's last pass). True when it is.
pub fn wait_group_gone(pgid: u32, max: Duration) -> bool {
    let started = Instant::now();
    while group_alive(pgid) {
        if started.elapsed() >= max {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10)); // sync-ok: the documented blocking wait
    }
    true
}

/// BLOCKING: waits until this process's child `pid` has ended, and leaves it unreaped (`waitid` with
/// `WNOWAIT`), so its pid cannot name another process until the caller reaps it. False when `pid` is
/// not a child of this process.
pub fn wait_ended_unreaped(pid: u32) -> bool {
    let id: libc::id_t = pid;
    loop {
        // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: a live siginfo_t and plain values.
        let r = unsafe { libc::waitid(libc::P_PID, id, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if r == 0 {
            return true;
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

/// BLOCKING, a moment: Flare's own pass over a helper's tree, after its guard ended or was killed.
/// Sends `SIGKILL` to every live process of the session or group `leader` leads, again until a pass
/// finds none (at most [`KILL_PASSES`]); returns how many it sent to. Never this process, launchd, or
/// a session or group this process is in. Call it while the leader is not yet reaped: its pid, and so
/// the session and group ids, cannot name anything else until then.
pub fn end_session(leader: u32) -> usize {
    let Some(lead) = libc::pid_t::try_from(leader).ok().filter(|l| *l > 1) else {
        return 0;
    };
    // SAFETY: no arguments, or plain values.
    let (me, my_sid, my_pgid) = unsafe { (libc::getpid(), libc::getsid(0), libc::getpgid(0)) };
    if lead == me || lead == my_sid || lead == my_pgid {
        return 0;
    }
    let mut total = 0;
    for _ in 0..KILL_PASSES {
        let mut sent = 0;
        for pid in crate::all_pids() {
            let Some(p) = libc::pid_t::try_from(pid)
                .ok()
                .filter(|p| *p > 1 && *p != me)
            else {
                continue;
            };
            // SAFETY: plain values in.
            let ours = unsafe { libc::getsid(p) == lead || libc::getpgid(p) == lead };
            // SAFETY: plain values in.
            if ours && crate::alive(pid) && unsafe { libc::kill(p, libc::SIGKILL) } == 0 {
                sent += 1;
            }
        }
        total += sent;
        if sent == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: the documented short wait
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::Stdio;

    /// The guards of this binary are this binary running [`guard_entry`].
    fn entry() {
        use_test_entry("guard::tests::guard_entry").unwrap();
    }

    /// Not a test on its own: a guard started by this binary's tests runs as this test.
    #[test]
    fn guard_entry() {
        run_if_guard();
    }

    /// Ends the listed pids if a test fails before its tree is gone, so a failure leaves nothing behind.
    struct Reap(Vec<u32>);
    impl Drop for Reap {
        fn drop(&mut self) {
            for p in &self.0 {
                crate::signal(*p, crate::Signal::Kill);
            }
        }
    }

    /// Runs a guarded `sh -c script` that writes the pids of its tree to `pids`, one per line, then
    /// `ready`; returns the child, its guard and the pids once `ready` is there.
    fn tree(dir: &std::path::Path, script: &str) -> (std::process::Child, Guard, Reap) {
        entry();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        let pending = guard_command(&mut cmd).unwrap();
        let child = cmd.spawn().unwrap();
        let guard = pending.started();
        let t = Instant::now();
        while !dir.join("ready").exists() {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "the tree did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let pids: Vec<u32> = std::fs::read_to_string(dir.join("pids"))
            .unwrap()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect();
        (child, guard, Reap(pids))
    }

    // The leader, a child, a job in a group of its own, an orphan (its subshell ends), in that order.
    const TREE: &str = "echo $$ >> pids; sleep 300 & echo $! >> pids; \
        set -m; sleep 300 & echo $! >> pids; set +m; \
        (sleep 300 & echo $! > orphan.tmp; mv orphan.tmp orphan) & \
        while [ ! -f orphan ]; do sleep 0.05; done; cat orphan >> pids; \
        touch ready; wait";

    fn all_gone(pids: &[u32]) -> bool {
        let t = Instant::now();
        while pids.iter().any(|p| crate::alive(*p)) {
            if t.elapsed() > Duration::from_secs(10) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// A script that starts an escapee (this binary's [`escapee`]: it leaves the session) and adds its
    /// pid to `pids`, then runs `then`.
    fn with_escapee(then: &str) -> String {
        let me = std::env::current_exe().unwrap();
        format!(
            "echo $$ >> pids; set -m; sleep 300 & echo $! >> pids; set +m; \
             FLARE_PROC_ESCAPE=\"$PWD\" '{}' guard::tests::escapee --exact --nocapture >/dev/null 2>&1 & \
             while [ ! -f escaped ]; do sleep 0.05; done; cat escaped >> pids; {then}",
            me.display()
        )
    }

    #[test]
    fn the_program_leads_a_new_session_and_its_guard_is_an_exec_d_orphan_with_a_group_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, mut guard, pids) = tree(dir.path(), TREE);
        let lead = child.id();
        assert_eq!(pids.0[0], lead);
        let st = crate::stat(lead).unwrap();
        assert_eq!((st.pgid, crate::session_of(lead)), (lead, Some(lead)));
        assert_eq!(st.tty, None, "no controlling terminal");
        // The `set -m` job left the group but not the session.
        let job = crate::stat(pids.0[2]).unwrap();
        assert_ne!(
            job.pgid, lead,
            "the job control child has a group of its own"
        );
        assert_eq!(crate::session_of(pids.0[2]), Some(lead));
        // The guard is in the session, an orphan (launchd's), not a child of the program, leads a
        // group of its own (rule 1053), and is a fresh exec of the entry, not a fork of this process
        // (rule 1054: it holds none of this process's memory).
        let id = guard.id().expect("the guard said its pid");
        let g = crate::stat(id.pid).unwrap();
        assert_eq!(crate::session_of(id.pid), Some(lead));
        assert_eq!((g.ppid, g.pgid), (1, id.pid));
        assert!(!crate::children(lead).contains(&id.pid));
        let argv = crate::argv(id.pid).unwrap();
        assert!(argv[0].starts_with(MARK), "{argv:?}");
        assert_eq!(argv[1], "guard::tests::guard_entry");
        assert!(id.alive());
        guard.release();
        child.wait().unwrap();
        assert!(all_gone(&pids.0));
        assert!(id.wait_gone(Duration::from_secs(10)));
    }

    #[test]
    fn releasing_the_guard_ends_the_group_the_session_and_the_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, mut guard, pids) = tree(dir.path(), TREE);
        assert_eq!(pids.0.len(), 4);
        assert!(pids.0.iter().all(|p| crate::alive(*p)));
        guard.release();
        child.wait().unwrap();
        assert!(all_gone(&pids.0), "{:?}", pids.0);
        assert!(wait_group_gone(pids.0[0], Duration::from_secs(10)));
    }

    #[test]
    fn a_program_that_ends_by_itself_takes_its_tree_with_it() {
        let dir = tempfile::tempdir().unwrap();
        // The program ends at once; its children are orphans in its group and session.
        let script = "echo $$ >> pids; sleep 300 & echo $! >> pids; \
            set -m; sleep 300 & echo $! >> pids; touch ready; sleep 1; exit 0";
        let (mut child, _guard, pids) = tree(dir.path(), script);
        child.wait().unwrap();
        assert!(all_gone(&pids.0), "{:?}", pids.0);
    }

    /// Rule 1053: a helper that kills its own group (`kill -9 0`, a common way to end a tree) does not
    /// take its guard with it, so what left the group (a `set -m` job) or the session (`setsid`) still
    /// ends. With the guard in the helper's group, both survived.
    #[test]
    fn a_program_that_kills_its_own_group_still_has_its_whole_tree_ended() {
        let dir = tempfile::tempdir().unwrap();
        let script = with_escapee("touch ready; sleep 2; kill -9 0");
        let (mut child, guard, pids) = tree(dir.path(), &script);
        assert_eq!(pids.0.len(), 3);
        child.wait().unwrap();
        assert!(all_gone(&pids.0), "{:?}", pids.0);
        let id = guard.id().unwrap();
        assert!(id.wait_gone(Duration::from_secs(10)));
    }

    #[test]
    fn a_descendant_that_left_the_session_is_ended_too() {
        let dir = tempfile::tempdir().unwrap();
        let script = with_escapee("touch ready; wait");
        let (mut child, mut guard, pids) = tree(dir.path(), &script);
        let esc = pids.0[2];
        assert_eq!(
            crate::session_of(esc),
            Some(esc),
            "it leads a session of its own"
        );
        // The escapee stays the shell's child, so the guard's next tick notes it.
        std::thread::sleep(TICK * 4);
        guard.release();
        child.wait().unwrap();
        assert!(all_gone(&pids.0), "{:?}", pids.0);
    }

    /// Flare's own pass ends the session and the group when the guard is gone (the helper killed it).
    #[test]
    fn end_session_ends_the_session_and_the_group_but_never_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, guard, pids) = tree(dir.path(), TREE);
        let id = guard.id().unwrap();
        assert!(crate::signal(id.pid, crate::Signal::Kill));
        assert!(id.wait_gone(Duration::from_secs(10)));
        // The guard is gone; the tree runs on. Flare's pass, while the leader is not reaped.
        assert!(pids.0.iter().all(|p| crate::alive(*p)));
        assert!(end_session(child.id()) >= 4);
        assert!(all_gone(&pids.0), "{:?}", pids.0);
        child.wait().unwrap();
        // SAFETY: no arguments.
        let (sid, pgid) = unsafe { (libc::getsid(0), libc::getpgid(0)) };
        for own in [sid, pgid, 0, 1] {
            assert_eq!(end_session(u32::try_from(own).unwrap()), 0);
        }
        drop(guard);
    }

    /// Not a test on its own: run by the escapee scripts with `FLARE_PROC_ESCAPE` set, it leaves its
    /// session, says its pid, and sleeps.
    #[test]
    fn escapee() {
        let Some(dir) = std::env::var_os("FLARE_PROC_ESCAPE") else {
            return;
        };
        // SAFETY: no arguments.
        unsafe { libc::setsid() };
        let dir = std::path::PathBuf::from(dir);
        std::fs::write(dir.join("escaped.tmp"), format!("{}\n", std::process::id())).unwrap();
        std::fs::rename(dir.join("escaped.tmp"), dir.join("escaped")).unwrap();
        std::thread::sleep(Duration::from_secs(300));
    }

    #[test]
    fn a_program_that_cannot_start_leaves_no_guard_behind() {
        entry();
        let mut cmd = Command::new("/nonexistent/program");
        let pending = guard_command(&mut cmd).unwrap();
        assert!(cmd.spawn().is_err());
        drop(pending);
        // The guard of the failed start saw its program end (the exec failed) and is gone: no
        // orphan of this binary is left in a session other than ours.
        let me = std::process::id();
        let comm = crate::stat(me).unwrap().comm;
        let ours = crate::session_of(me);
        let t = Instant::now();
        while crate::table().iter().any(|r| {
            r.ppid == 1 && r.comm == comm && r.uid == crate::own_uid() && {
                let s = crate::session_of(r.pid);
                s != ours && s.is_some_and(|s| crate::stat(s).is_none())
            }
        }) {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "a guard was left behind"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_output_of_a_guarded_program_closes_when_its_tree_ends() {
        // The guard closes its copies of the program's descriptors: a reader sees the end.
        entry();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo hi; sleep 300 & sleep 300")
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        let pending = guard_command(&mut cmd).unwrap();
        let mut child = cmd.spawn().unwrap();
        let mut guard = pending.started();
        let mut out = child.stdout.take().unwrap();
        let mut first = [0u8; 3];
        out.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"hi\n");
        guard.release();
        let mut rest = Vec::new();
        out.read_to_end(&mut rest).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn the_mark_is_written_without_allocating_and_read_back_strictly() {
        let mut buf = [0xffu8; 64];
        write_mark(&mut buf, 7, 12, 40321);
        let end = buf.iter().position(|b| *b == 0).unwrap();
        assert_eq!(&buf[..end], b"flare-guard:7:12:40321");
        let rest = &buf[MARK.len()..end];
        assert_eq!(parse_mark(rest), Some((7, 12, 40321)));
        write_mark(&mut buf, i32::MAX, 3, i32::MAX);
        let end = buf.iter().position(|b| *b == 0).unwrap();
        assert_eq!(
            parse_mark(&buf[MARK.len()..end]),
            Some((i32::MAX, 3, i32::MAX))
        );
        for bad in [
            "",
            "7:12",
            "7:12:40321:1",
            "7:12:1",
            "2:12:400",
            "7:7:400",
            "7:-12:400",
            "7:12:+400",
            "7:12:x",
            "7::400",
            "99999999999:12:400",
        ] {
            assert_eq!(parse_mark(bad.as_bytes()), None, "{bad}");
        }
    }

    /// A start in guard mode that is not a real guard's (its `argv[0]` set by hand) refuses at once and
    /// signals nothing: the descriptors are not its, and its session leader is not Flare's child.
    #[test]
    fn a_stray_guard_start_refuses_and_ends_nothing() {
        let me = std::env::current_exe().unwrap();
        let mut sleeper = Command::new("/bin/sleep").arg("300").spawn().unwrap();
        let pid = sleeper.id();
        let status = Command::new(&me)
            .arg0(format!("{MARK}3:4:{pid}"))
            .args(["guard::tests::guard_entry", "--exact", "--nocapture"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
        assert!(crate::alive(pid));
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
    }
}
