//! The engine's helpers other than the renderers: the GPU process and the utilities (the network service among
//! them). CEF says when a renderer ends (`on_render_process_terminated`) and has no callback for any other helper,
//! so the browser process watches its own children: a helper is known by its `--type=` switch (read from the
//! process's arguments, `KERN_PROCARGS2`), its end comes from the kernel (`EVFILT_PROC`, `NOTE_EXIT` with the wait
//! status), and only an end that is a failure (a signal or a non-zero exit) while the engine runs is reported.
//! Chromium starts the GPU process and the utilities again by itself, so a report is a note for the log, as on
//! Windows (`Failure::Recovers`): nothing is restarted here (rule 1158).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// A helper that ended in failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperExit {
    pub pid: i32,
    /// `gpu` or `utility` (the Windows host's words for the same processes).
    pub kind: &'static str,
    /// The utility's service (`network.mojom.NetworkService`), empty for the GPU process.
    pub service: String,
    /// The raw wait status.
    pub status: i32,
}

impl HelperExit {
    /// Ended by a signal or with a non-zero exit code (a utility that finished its work exits 0).
    pub fn failed(&self) -> bool {
        self.status != 0
    }

    /// The reason in WebView2's words: `crashed` (a signal), `unexpected` (an exit code).
    pub fn reason(&self) -> &'static str {
        if libc::WIFSIGNALED(self.status) {
            "crashed"
        } else {
            "unexpected"
        }
    }

    /// The signal, or the exit code.
    pub fn code(&self) -> i32 {
        if libc::WIFSIGNALED(self.status) {
            libc::WTERMSIG(self.status)
        } else {
            libc::WEXITSTATUS(self.status)
        }
    }
}

/// What kind of helper a process with these arguments is: `gpu` or `utility` (with its service), none for a renderer
/// (CEF reports those itself) or anything that is not a CEF helper.
pub fn kind_of(args: &[String]) -> Option<(&'static str, String)> {
    let switch = |name: &str| {
        args.iter()
            .find_map(|a| a.strip_prefix(name).map(str::to_string))
    };
    match switch("--type=")?.as_str() {
        "gpu-process" => Some(("gpu", String::new())),
        "utility" => Some(("utility", switch("--utility-sub-type=").unwrap_or_default())),
        _ => None,
    }
}

/// A process's arguments (`KERN_PROCARGS2`: argc, the executable's path, padding, then argc strings).
fn args_of(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: `mib` is a valid three-part name; a null buffer asks for the size only.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` holds `size` bytes, the length passed.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    parse_procargs(buf.get(..size)?)
}

fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let rest = buf.get(4..)?;
    // The executable's path, then the NULs that pad it.
    let path_end = rest.iter().position(|&b| b == 0)?;
    let mut i = path_end;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    let mut args = Vec::new();
    for part in rest.get(i..)?.split(|&b| b == 0) {
        if args.len() >= usize::try_from(argc).ok()? {
            break;
        }
        args.push(String::from_utf8_lossy(part).into_owned());
    }
    Some(args)
}

/// This process's children.
fn children() -> Vec<i32> {
    let mut pids = vec![0i32; 256];
    loop {
        let bytes = i32::try_from(pids.len() * 4).unwrap_or(i32::MAX);
        // SAFETY: `pids` holds `bytes` bytes, the length passed.
        let n = unsafe {
            libc::proc_listchildpids(std::process::id() as i32, pids.as_mut_ptr().cast(), bytes)
        };
        if n <= 0 {
            return Vec::new();
        }
        // The count is in pids on macOS (in bytes on older systems): either way a full buffer means look again.
        let count = usize::try_from(n).unwrap_or(0).min(pids.len());
        if count < pids.len() {
            pids.truncate(count);
            pids.retain(|&p| p > 0);
            return pids;
        }
        pids.resize(pids.len() * 2, 0);
    }
}

const HINT: usize = 1;
const STOP: usize = 2;
/// After a hint (a helper is about to start), the children are looked at this many times, [`LOOK_EVERY`] apart.
const LOOKS: u32 = 40;
const LOOK_EVERY: Duration = Duration::from_millis(50);

struct Kq(i32);

impl Kq {
    fn user(&self, ident: usize, flags: u16, fflags: u32) -> bool {
        let ev = libc::kevent {
            ident,
            filter: libc::EVFILT_USER,
            flags,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no event list.
        unsafe { libc::kevent(self.0, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
    }
}

impl Drop for Kq {
    fn drop(&mut self) {
        // SAFETY: the queue this owns, closed once.
        unsafe { libc::close(self.0) };
    }
}

/// Tells a [`Watcher`] that a helper is about to start (CEF's `on_before_child_process_launch`).
#[derive(Clone)]
pub struct Hint(Arc<Kq>);

impl Hint {
    pub fn launching(&self) {
        self.0.user(HINT, 0, libc::NOTE_TRIGGER);
    }
}

/// Watches this process's GPU and utility helpers and calls `on_exit` for each that ends in failure, until
/// [`Watcher::ending`] (the engine shuts down and ends them itself) or the watcher is dropped.
pub struct Watcher {
    kq: Arc<Kq>,
    ending: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching (and looks at the children already there). None when the kernel gives no queue.
    pub fn start(on_exit: Arc<dyn Fn(HelperExit) + Send + Sync>) -> Option<Watcher> {
        // SAFETY: no arguments; the result is checked.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return None;
        }
        let kq = Arc::new(Kq(fd));
        if !kq.user(HINT, libc::EV_ADD | libc::EV_CLEAR, 0)
            || !kq.user(STOP, libc::EV_ADD | libc::EV_CLEAR, 0)
        {
            return None;
        }
        let ending: Arc<AtomicBool> = Arc::default();
        let (q, e) = (Arc::clone(&kq), Arc::clone(&ending));
        let thread = std::thread::Builder::new()
            .name("flare-cef-helpers".into())
            .spawn(move || watch(&q, &e, &*on_exit))
            .ok()?;
        Some(Watcher {
            kq,
            ending,
            thread: Some(thread),
        })
    }

    pub fn hint(&self) -> Hint {
        Hint(Arc::clone(&self.kq))
    }

    /// The engine is shutting down: the helpers it ends now are not failures.
    pub fn ending(&self) {
        self.ending.store(true, Ordering::SeqCst);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.ending();
        self.kq.user(STOP, 0, libc::NOTE_TRIGGER);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch(kq: &Kq, ending: &AtomicBool, on_exit: &(dyn Fn(HelperExit) + Send + Sync)) {
    let mut watched: HashMap<i32, (&'static str, String)> = HashMap::new();
    let mut known: HashSet<i32> = HashSet::new();
    let mut looks = LOOKS;
    // SAFETY: `kevent` is plain data; zeroed is a valid empty event.
    let mut evs: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
    loop {
        if looks > 0 {
            looks -= 1;
            let now = children();
            known.retain(|p| now.contains(p));
            for pid in now {
                if known.contains(&pid) {
                    continue;
                }
                // A child read before its exec still has this process's arguments (or none yet): it stays unknown
                // and is read again at the next look, never taken for good as no helper.
                let Some(kind) = args_of(pid).as_deref().and_then(kind_of) else {
                    continue;
                };
                known.insert(pid);
                let ev = libc::kevent {
                    ident: pid as usize,
                    filter: libc::EVFILT_PROC,
                    flags: libc::EV_ADD | libc::EV_ONESHOT,
                    fflags: libc::NOTE_EXIT | libc::NOTE_EXITSTATUS,
                    data: 0,
                    udata: std::ptr::null_mut(),
                };
                // SAFETY: one valid change, no event list. Fails (ESRCH) for a helper already gone.
                if unsafe { libc::kevent(kq.0, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) }
                    == 0
                {
                    watched.insert(pid, kind);
                }
            }
        }
        let wait = libc::timespec {
            tv_sec: 0,
            tv_nsec: LOOK_EVERY.as_nanos() as libc::c_long,
        };
        let timeout = if looks > 0 {
            &wait as *const libc::timespec
        } else {
            std::ptr::null()
        };
        // SAFETY: `evs` holds 8 events, the count passed; no changes.
        let n = unsafe { libc::kevent(kq.0, std::ptr::null(), 0, evs.as_mut_ptr(), 8, timeout) };
        for ev in evs.iter().take(usize::try_from(n).unwrap_or(0)) {
            match ev.filter {
                libc::EVFILT_USER if ev.ident == STOP => return,
                libc::EVFILT_USER => looks = LOOKS,
                libc::EVFILT_PROC => {
                    let pid = ev.ident as i32;
                    known.remove(&pid);
                    let Some((kind, service)) = watched.remove(&pid) else {
                        continue;
                    };
                    let exit = HelperExit {
                        pid,
                        kind,
                        service,
                        status: ev.data as i32,
                    };
                    if exit.failed() && !ending.load(Ordering::SeqCst) {
                        on_exit(exit);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::Instant;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_helper_is_known_by_its_type_switch_and_a_renderer_is_left_to_cef() {
        assert_eq!(
            kind_of(&s(&["helper", "--type=gpu-process", "--foo"])),
            Some(("gpu", String::new()))
        );
        assert_eq!(
            kind_of(&s(&[
                "helper",
                "--type=utility",
                "--utility-sub-type=network.mojom.NetworkService"
            ])),
            Some(("utility", "network.mojom.NetworkService".to_string()))
        );
        assert_eq!(kind_of(&s(&["helper", "--type=renderer"])), None);
        assert_eq!(kind_of(&s(&["/bin/zsh", "-l"])), None);
    }

    #[test]
    fn a_signal_is_a_crash_an_exit_code_is_unexpected_and_a_clean_exit_no_failure() {
        let e = |status| HelperExit {
            pid: 1,
            kind: "gpu",
            service: String::new(),
            status,
        };
        assert!(!e(0).failed());
        assert_eq!((e(9).reason(), e(9).code()), ("crashed", 9));
        assert_eq!((e(3 << 8).reason(), e(3 << 8).code()), ("unexpected", 3));
        assert!(e(3 << 8).failed());
    }

    #[test]
    fn the_arguments_of_a_process_read_back() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/x/helper\0\0\0\0helper\0--type=utility\0--a\0ENV=1\0");
        assert_eq!(
            parse_procargs(&buf),
            Some(s(&["helper", "--type=utility", "--a"]))
        );
    }

    /// A child that looks like a helper (its `--type=`) ends: a crash is reported with its kind, a clean exit and
    /// anything after `ending` are not.
    #[test]
    fn a_helper_that_crashes_is_reported_and_one_that_exits_cleanly_or_at_shutdown_is_not() {
        let seen: Arc<Mutex<Vec<HelperExit>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        let w = Watcher::start(Arc::new(move |e| s2.lock().unwrap().push(e))).expect("a watcher");
        let spawn = |script: &str, ty: &str| {
            Command::new("/bin/sh")
                .args(["-c", script, "sh", ty])
                .spawn()
                .expect("a child")
        };
        let mut crash = spawn("sleep 0.5; kill -KILL $$", "--type=gpu-process");
        let mut clean = spawn("sleep 0.5; exit 0", "--type=utility");
        w.hint().launching();
        let (crash_pid, clean_pid) = (crash.id() as i32, clean.id() as i32);
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !seen.lock().unwrap().iter().any(|e| e.pid == crash_pid) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = (crash.wait(), clean.wait());
        std::thread::sleep(Duration::from_millis(200));
        let got: Vec<HelperExit> = seen.lock().unwrap().clone();
        let mine = got
            .iter()
            .find(|e| e.pid == crash_pid)
            .expect("the crash is reported");
        assert_eq!(
            (mine.kind, mine.reason(), mine.code()),
            ("gpu", "crashed", 9)
        );
        assert!(!got.iter().any(|e| e.pid == clean_pid), "{got:?}");

        w.ending();
        let mut late = spawn("sleep 0.3; kill -KILL $$", "--type=gpu-process");
        w.hint().launching();
        let late_pid = late.id() as i32;
        let _ = late.wait();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!seen.lock().unwrap().iter().any(|e| e.pid == late_pid));
        drop(w);
    }
}
