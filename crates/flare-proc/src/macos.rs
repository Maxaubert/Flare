//! macOS: libproc for the table, the working folder and the executable, sysctl `KERN_PROCARGS2`
//! for argv, kqueue `NOTE_EXIT` for the exit wait. Every call works on the user's own processes
//! without an entitlement (Flare ships unsandboxed with a Developer ID). Nothing here blocks except
//! `wait_for_exit`, which says so.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// `NODEV`: the controlling-terminal field of a process that has none.
const NODEV: u32 = u32::MAX;

/// One process, as `proc_pidinfo(PROC_PIDTBSDINFO)` reports it. Of another user's process (root's
/// daemons, launchd) the kernel gives only the short form: no start time and no terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: u32,
    pub ppid: u32,
    /// The process group.
    pub pgid: u32,
    pub uid: u32,
    /// When the process started, microseconds since the Unix epoch. With the pid it names one
    /// process for good: a recycled pid has a later start. None for another user's process.
    pub start_us: Option<u64>,
    /// The controlling terminal (a device number); None when the process has none.
    pub tty: Option<u32>,
    /// The foreground process group of that terminal; None without a terminal.
    pub tpgid: Option<u32>,
    /// The short command name (at most 16 bytes, the executable's file name).
    pub comm: String,
}

/// A signal Flare sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Hup,
    Term,
    Kill,
}

impl Signal {
    fn raw(self) -> libc::c_int {
        match self {
            Signal::Hup => libc::SIGHUP,
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }
}

fn to_pid(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)
}

/// Every pid on the system (the kernel's own 0 left out). Empty when the list cannot be read.
pub fn all_pids() -> Vec<u32> {
    // SAFETY: a null buffer asks only for the count, in bytes.
    let need = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(need) = usize::try_from(need) else {
        return Vec::new();
    };
    // Room for processes started between the two calls.
    let mut buf: Vec<libc::pid_t> = vec![0; need + 64];
    let bytes = libc::c_int::try_from(buf.len() * std::mem::size_of::<libc::pid_t>())
        .unwrap_or(libc::c_int::MAX);
    // SAFETY: `buf` holds `bytes` bytes; the call writes at most that many and returns the count.
    let n = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast(), bytes) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    buf.truncate(n);
    buf.into_iter()
        .filter_map(|p| u32::try_from(p).ok())
        .filter(|p| *p != 0)
        .collect()
}

/// The direct children of `pid` (libproc `proc_listchildpids`). Empty when there are none or the
/// list cannot be read.
pub fn children(pid: u32) -> Vec<u32> {
    let Some(p) = to_pid(pid) else {
        return Vec::new();
    };
    let mut buf: Vec<libc::pid_t> = vec![0; 256];
    loop {
        let bytes = libc::c_int::try_from(buf.len() * std::mem::size_of::<libc::pid_t>())
            .unwrap_or(libc::c_int::MAX);
        // SAFETY: `buf` holds `bytes` bytes; the call writes at most that many and returns the count.
        let n = unsafe { libc::proc_listchildpids(p, buf.as_mut_ptr().cast(), bytes) };
        let Ok(n) = usize::try_from(n) else {
            return Vec::new();
        };
        // The count is in pids on current systems; a full buffer may have been cut, so grow it.
        if n < buf.len() || buf.len() >= 1 << 16 {
            buf.truncate(n.min(buf.len()));
            return buf
                .into_iter()
                .filter_map(|c| u32::try_from(c).ok())
                .collect();
        }
        buf.resize(buf.len() * 4, 0);
    }
}

/// One process's table row; None when it is gone or a zombie.
pub fn stat(pid: u32) -> Option<ProcStat> {
    let p = to_pid(pid)?;
    full_stat(p).or_else(|| short_stat(p))
}

fn full_stat(p: libc::pid_t) -> Option<ProcStat> {
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `info` is a live struct of `size` bytes, the size this flavor fills.
    let got =
        unsafe { libc::proc_pidinfo(p, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if got != size || info.pbi_status == libc::SZOMB {
        return None;
    }
    let comm: Vec<u8> = info.pbi_comm.iter().map(|&c| c as u8).collect();
    Some(ProcStat {
        pid: info.pbi_pid,
        ppid: info.pbi_ppid,
        pgid: info.pbi_pgid,
        uid: info.pbi_uid,
        start_us: Some(
            info.pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec),
        ),
        tty: (info.e_tdev != NODEV && info.e_tdev != 0).then_some(info.e_tdev),
        tpgid: (info.e_tdev != NODEV && info.e_tpgid != 0).then_some(info.e_tpgid),
        comm: crate::c_field(&comm),
    })
}

/// `PROC_PIDT_SHORTBSDINFO`, which the kernel answers for every process that exists (not a zombie).
fn short_info(p: libc::pid_t) -> Option<libc::proc_bsdshortinfo> {
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdshortinfo>()).ok()?;
    // SAFETY: `info` is a live struct of `size` bytes, the size this flavor fills.
    let got = unsafe {
        libc::proc_pidinfo(
            p,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    (got == size && info.pbsi_status != libc::SZOMB).then_some(info)
}

fn short_stat(p: libc::pid_t) -> Option<ProcStat> {
    let info = short_info(p)?;
    let comm: Vec<u8> = info.pbsi_comm.iter().map(|&c| c as u8).collect();
    Some(ProcStat {
        pid: info.pbsi_pid,
        ppid: info.pbsi_ppid,
        pgid: info.pbsi_pgid,
        uid: info.pbsi_uid,
        start_us: None,
        tty: None,
        tpgid: None,
        comm: crate::c_field(&comm),
    })
}

/// Every live process's row. Processes that end while it is read, and zombies, are left out.
pub fn table() -> Vec<ProcStat> {
    all_pids().into_iter().filter_map(stat).collect()
}

/// The session `pid` belongs to (its session leader's pid); None when it is gone.
pub fn session_of(pid: u32) -> Option<u32> {
    let p = to_pid(pid)?;
    // SAFETY: plain value in; -1 means failure.
    let s = unsafe { libc::getsid(p) };
    u32::try_from(s).ok()
}

/// The working folder of `pid` (`PROC_PIDVNODEPATHINFO`); None when it is gone or not ours.
pub fn cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let p = to_pid(pid)?;
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
    // SAFETY: `info` is a live struct of `size` bytes, the size this flavor fills.
    let got = unsafe {
        libc::proc_pidinfo(
            p,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if got != size {
        return None;
    }
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|&c| c as u8)
        .collect();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    (end > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

/// The full path of `pid`'s executable (`proc_pidpath`); None when it is gone or not readable.
pub fn exe(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let p = to_pid(pid)?;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = u32::try_from(buf.len()).ok()?;
    // SAFETY: `buf` holds `len` bytes; the call writes at most that many and returns the length.
    let n = unsafe { libc::proc_pidpath(p, buf.as_mut_ptr().cast(), len) };
    let n = usize::try_from(n).ok().filter(|n| *n > 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(
        &buf[..n.min(buf.len())],
    )))
}

/// The argv of `pid` (sysctl `KERN_PROCARGS2`); None when it is gone or not ours.
pub fn argv(pid: u32) -> Option<Vec<String>> {
    let p = to_pid(pid)?;
    let mut max: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` names a 2-level int sysctl; `max` is a live int and `len` its size.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut max).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    let max = usize::try_from(max)
        .ok()
        .filter(|_| ok == 0)
        .unwrap_or(1 << 20);
    let mut buf = vec![0u8; max];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, p];
    // SAFETY: `mib` names a 3-level sysctl; `buf` holds `size` bytes and `size` is updated to the count written.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 {
        return None;
    }
    crate::parse_procargs2(&buf[..size.min(buf.len())]).map(|(_, a)| a)
}

/// True while `pid` runs. A zombie (ended, not yet reaped) is not running.
pub fn alive(pid: u32) -> bool {
    let Some(p) = to_pid(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; kill only checks that the pid exists.
    let exists = unsafe { libc::kill(p, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !is_zombie(p)
}

fn is_zombie(p: libc::pid_t) -> bool {
    // A zombie has no task left to describe, so the kernel answers nothing even in the short form,
    // which it gives for any live process, another user's included.
    short_info(p).is_none()
}

/// Sends `sig` to `pid`. False when it was not delivered (gone, or not ours).
pub fn signal(pid: u32, sig: Signal) -> bool {
    let Some(p) = to_pid(pid) else {
        return false;
    };
    // SAFETY: plain values in.
    unsafe { libc::kill(p, sig.raw()) == 0 }
}

/// Sends `sig` to every process in group `pgid` (`killpg`). False when nothing got it.
pub fn signal_group(pgid: u32, sig: Signal) -> bool {
    // Group 1 is launchd's; 0 would mean the caller's own group.
    let Some(g) = to_pid(pgid).filter(|g| *g > 1) else {
        return false;
    };
    // SAFETY: plain values in.
    unsafe { libc::killpg(g, sig.raw()) == 0 }
}

/// A close-on-exec duplicate of `fd` (a shell started later must not inherit it); None on failure.
pub fn dup_fd(fd: std::os::fd::RawFd) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain values in; -1 means failure.
    let d = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    // SAFETY: `d` is a fresh descriptor this call owns, closed once by the OwnedFd.
    (d >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(d) })
}

/// Throws away the output a terminal holds that nobody has read (`tcflush(TCOFLUSH)` on the pty
/// master). A process that ends closes its terminal, and macOS makes that close wait until the
/// output is read: with the tab's reader stopped, a dying shell would hang in its exit for good.
pub fn discard_output(master: std::os::fd::BorrowedFd<'_>) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: a live descriptor (borrowed for the call) and a plain value.
    unsafe { libc::tcflush(master.as_raw_fd(), libc::TCOFLUSH) == 0 }
}

/// BLOCKING, at most `max`. Waits until `pid` has ended; true when it is gone (or never was). A
/// kqueue `NOTE_EXIT` watch; a poll of `alive` only when the watch cannot be set.
pub fn wait_for_exit(pid: u32, max: Duration) -> bool {
    let Some(p) = to_pid(pid) else {
        return true;
    };
    let started = Instant::now();
    // SAFETY: no arguments; -1 means failure.
    let kq = unsafe { libc::kqueue() };
    if kq >= 0 {
        let answer = watch_exit(kq, p, max);
        // SAFETY: `kq` is ours and closed once.
        unsafe { libc::close(kq) };
        if let Some(gone) = answer {
            return gone;
        }
    }
    loop {
        if !alive(pid) {
            return true;
        }
        let left = max.saturating_sub(started.elapsed());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(20))); // sync-ok: the documented blocking wait, a fallback poll
    }
}

/// The kqueue half of `wait_for_exit`: Some(answer), or None when the watch could not be set.
fn watch_exit(kq: libc::c_int, p: libc::pid_t, max: Duration) -> Option<bool> {
    let ident = usize::try_from(p).ok()?;
    let ev = libc::kevent {
        ident,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: one live change in, no events out, no timeout.
    let r = unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if r < 0 {
        // ESRCH: it has already ended (or is a zombie, which has).
        return (std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
            .then_some(true);
    }
    // It may have ended between the check the caller made and the watch.
    if !alive(u32::try_from(p).ok()?) {
        return Some(true);
    }
    let started = Instant::now();
    loop {
        let left = max.saturating_sub(started.elapsed());
        let ts = libc::timespec {
            tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
            tv_nsec: libc::c_long::from(left.subsec_nanos()),
        };
        // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: no changes in, room for one event out, a live timeout.
        let n = unsafe { libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, &ts) };
        if n > 0 {
            return Some(true);
        }
        if n == 0 {
            return Some(false);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return None;
        }
    }
}

/// A process's memory as the kernel counts it (`proc_pid_rusage`, `RUSAGE_INFO_V2`), in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// The resident set: the pages in RAM now (Windows' working set).
    pub resident: u64,
    /// The physical footprint: what Activity Monitor shows as Memory, the process's own dirty and
    /// compressed pages (the nearest to Windows' private bytes).
    pub footprint: u64,
}

/// `pid`'s memory; None when it is gone or the kernel refuses (another user's process).
pub fn memory(pid: u32) -> Option<Memory> {
    let p = to_pid(pid)?;
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a live rusage_info_v2, the struct this flavor fills.
    let r = unsafe { libc::proc_pid_rusage(p, libc::RUSAGE_INFO_V2, (&raw mut info).cast()) };
    (r == 0).then_some(Memory {
        resident: info.ri_resident_size,
        footprint: info.ri_phys_footprint,
    })
}

/// This process's memory.
pub fn own_memory() -> Option<Memory> {
    memory(std::process::id())
}

/// The installed RAM in bytes (`sysctl hw.memsize`); None when it cannot be read.
pub fn installed_ram() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: a NUL-terminated name, an out-buffer of `len` bytes and its length; no new value.
    let r = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && len == std::mem::size_of::<u64>() && bytes > 0).then_some(bytes)
}

/// The real user id of this process.
pub fn own_uid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sleeper(secs: &str) -> std::process::Child {
        Command::new("/bin/sleep")
            .arg(secs)
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn this_process_is_in_the_table_with_its_parent_and_start() {
        let me = std::process::id();
        let t = table();
        let row = t.iter().find(|r| r.pid == me).expect("self in the table");
        assert_eq!(row.ppid, std::os::unix::process::parent_id());
        assert!(
            row.start_us.is_some_and(|s| s > 1_600_000_000_000_000),
            "{:?}",
            row.start_us
        );
        // Another user's process is listed too, in the short form.
        let launchd = stat(1).expect("launchd is listed");
        assert_eq!(
            (launchd.ppid, launchd.start_us, launchd.tty),
            (0, None, None)
        );
        assert!(
            alive(1),
            "a root process is alive though it cannot be signalled"
        );
        assert!(t.iter().any(|r| r.pid == 1));
        assert!(t.len() > 10);
        assert!(all_pids().contains(&me));
        assert_eq!(stat(me).unwrap().pgid, {
            // SAFETY: no arguments.
            u32::try_from(unsafe { libc::getpgrp() }).unwrap()
        });
    }

    #[test]
    fn a_child_is_listed_under_its_parent_with_its_argv_exe_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Command::new("/bin/sleep")
            .arg("30")
            .current_dir(dir.path())
            .spawn()
            .unwrap();
        let pid = c.id();
        let kids = children(std::process::id());
        let a = argv(pid);
        let e = exe(pid);
        let w = cwd(pid);
        let s = stat(pid);
        c.kill().unwrap();
        c.wait().unwrap();
        assert!(kids.contains(&pid), "{kids:?}");
        assert_eq!(a.unwrap(), ["/bin/sleep", "30"]);
        assert_eq!(e.unwrap(), PathBuf::from("/bin/sleep"));
        assert_eq!(
            w.unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let s = s.unwrap();
        assert_eq!(s.comm, "sleep");
        assert_eq!(s.ppid, std::process::id());
    }

    #[test]
    fn a_zombie_is_not_alive_and_a_reaped_pid_is_gone() {
        let mut c = sleeper("30");
        let pid = c.id();
        assert!(alive(pid));
        assert!(signal(pid, Signal::Kill));
        // Not reaped yet: a zombie, which has ended.
        assert!(wait_for_exit(pid, Duration::from_secs(5)));
        assert!(!alive(pid));
        assert!(stat(pid).is_none());
        c.wait().unwrap();
        assert!(!alive(pid));
        assert!(!signal(pid, Signal::Term));
        assert!(wait_for_exit(pid, Duration::from_millis(10)));
    }

    #[test]
    fn the_exit_wait_times_out_on_a_live_process_and_wakes_on_its_end() {
        let me = std::process::id();
        let t = Instant::now();
        assert!(!wait_for_exit(me, Duration::from_millis(100)));
        assert!(t.elapsed() >= Duration::from_millis(90));
        let mut c = sleeper("30");
        let pid = c.id();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            signal(pid, Signal::Term)
        });
        let t = Instant::now();
        assert!(wait_for_exit(pid, Duration::from_secs(10)));
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(killer.join().unwrap());
        c.wait().unwrap();
    }

    #[test]
    fn a_group_signal_reaches_every_member() {
        use std::os::unix::process::CommandExt;
        let mut lead = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let g = lead.id();
        let mut member = Command::new("/bin/sleep")
            .arg("30")
            .process_group(i32::try_from(g).unwrap())
            .spawn()
            .unwrap();
        assert_eq!(stat(member.id()).unwrap().pgid, g);
        assert!(signal_group(g, Signal::Term));
        lead.wait().unwrap();
        member.wait().unwrap();
        assert!(!signal_group(g, Signal::Term));
        assert!(
            !signal_group(1, Signal::Term),
            "launchd's group is never signalled"
        );
        assert!(!signal_group(0, Signal::Term));
    }

    #[test]
    fn a_new_session_has_its_leader_as_id_and_a_bad_pid_reads_nothing() {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        // SAFETY: setsid is async-signal-safe, the only call between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        let s = session_of(pid);
        let st = stat(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(s, Some(pid));
        let st = st.unwrap();
        assert_eq!(st.pgid, pid);
        assert_eq!(st.tty, None, "a new session has no terminal");
        assert_eq!(st.tpgid, None);
        assert_eq!(
            session_of(std::process::id()),
            // SAFETY: plain value in.
            u32::try_from(unsafe { libc::getsid(0) }).ok()
        );
        assert!(stat(0).is_none() && cwd(0).is_none() && argv(0).is_none());
        assert!(stat(u32::MAX).is_none() && exe(u32::MAX).is_none());
        assert!(children(0).is_empty());
    }

    #[test]
    fn this_process_has_a_resident_set_and_a_footprint() {
        let m = own_memory().expect("the kernel reports our own memory");
        assert!(m.resident > 1024 * 1024, "{m:?}");
        assert!(m.footprint > 0, "{m:?}");
        assert!(memory(0).is_none());
    }

    #[test]
    fn the_installed_ram_is_at_least_a_gigabyte() {
        let ram = installed_ram().expect("hw.memsize");
        assert!(ram >= 1 << 30, "{ram}");
    }

    #[test]
    fn our_uid_is_the_uid_our_row_shows() {
        let row = stat(std::process::id()).expect("our own row");
        assert_eq!(row.uid, own_uid());
    }
}
