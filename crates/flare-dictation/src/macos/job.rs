//! The reaping job on macOS (port phase 7): what the Windows kill-on-close Job Object does for every
//! dictation child, made of a process group per child and one reaper process.
//!
//! - **Groups.** Every engine child is spawned as the leader of its own process group
//!   ([`isolate`], `posix_spawn` with `setpgid(0, 0)`), so the group id is its pid and anything it starts
//!   lands in the same group. A terminal's Ctrl+C on a development build reaches neither it nor the reaper.
//! - **The reaper.** [`Job::new`] starts `/bin/sh` running [`REAPER`], in a group of its own too, with a
//!   pipe on its stdin and nothing else. [`Job::assign`] writes `+ <pid>` to the pipe; a child seen gone
//!   is written as `- <pid>` at the next assign. When the pipe closes, the reaper sends `SIGKILL` to every
//!   group it still holds and exits. The pipe closes when the job is dropped AND when Flare dies in any
//!   way (a crash, `kill -9`, a force quit): the kernel closes a dead process's descriptors, which is the
//!   one guarantee a library has without an entitlement (no `PR_SET_PDEATHSIG` on Darwin).
//! - **Refusals.** A job whose reaper is not running refuses `assign`, and the callers never run a child
//!   they cannot reap (rule 1081): the same contract as a job that cannot be made on Windows.
//!
//! A pid is only ever told to the reaper while the child behind it has not been waited on (a zombie keeps
//! its pid), and it is withdrawn BEFORE it is waited on: every engine child is waited through its
//! [`Member`], which sees the exit without reaping (`waitid` with `WNOWAIT`), writes `- <pid>` while the
//! child is still a zombie, and only then reaps it. So the reaper never holds a pid that is free for the
//! kernel to give to another process (rule 1087). The lazy withdrawal at the next assign stays as a backstop.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// The reaper's whole program (POSIX sh). It holds the pids it is told, and when its stdin ends it kills each
/// one's group and the pid itself. It also watches its parent (Flare) every 2 s and reaps when Flare is gone:
/// std sets close-on-exec on the pipe after `pipe()`, not atomically, so a process another thread spawns at that
/// moment can inherit the write end and keep the pipe open after Flare died (rule 1088). Every `+` and `-` line is
/// answered `ok` on stdout once it is held, and Flare waits for the answer: a pid is in the reaper's list before
/// `assign` returns, so a reap the watcher starts never misses a line still in the pipe. SIGPIPE is ignored too:
/// an answer to a Flare that is gone must not end the reaper before it reaps. Hang-ups and interrupts are ignored: a closing terminal must not end it
/// before it has done its work. A line that is not `+ <digits>` or `- <digits>` is ignored.
pub const REAPER: &str = r#"trap '' HUP INT PIPE
p=' '
w=
reap() {
  for x in $p; do
    if [ "$x" -gt 1 ]; then kill -9 -- "-$x" 2>/dev/null; kill -9 "$x" 2>/dev/null; fi
  done
  if [ -n "$w" ]; then kill "$w" 2>/dev/null; fi
  exit 0
}
trap reap USR1
o=$PPID
( while kill -0 "$o" 2>/dev/null; do sleep 2; done; kill -USR1 $$ 2>/dev/null ) </dev/null &
w=$!
while read -r op pid; do
  case $pid in ''|*[!0-9]*) continue ;; esac
  case $op in
    +) case $p in *" $pid "*) ;; *) p="$p$pid " ;; esac; echo ok ;;
    -) case $p in *" $pid "*) p="${p% $pid *} ${p#* $pid }" ;; esac; echo ok ;;
  esac
done
reap
"#;

/// The shell that runs the reaper: the system's own, never one found on PATH.
const SHELL: &str = "/bin/sh";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Reaper {
    child: Child,
    stdin: ChildStdin,
    /// The reaper's `ok` per line it holds.
    acks: BufReader<ChildStdout>,
    /// The pids told to the reaper and not withdrawn.
    held: Vec<u32>,
}

impl Reaper {
    /// Writes `lines` and waits for the reaper's `ok` to each of the `n` of them: held once this returns.
    fn tell(&mut self, lines: &str, n: usize) -> io::Result<()> {
        self.stdin.write_all(lines.as_bytes())?;
        self.stdin.flush()?;
        let mut ack = String::new();
        for _ in 0..n {
            ack.clear();
            // sync-ok: the reaper answers each line at once; a dead reaper is end of file
            if self.acks.read_line(&mut ack)? == 0 || ack.trim_end() != "ok" {
                return Err(io::Error::other("the reaper did not hold the pid"));
            }
        }
        Ok(())
    }
}

/// The reaping job: one reaper, the pids it holds.
pub struct Job {
    reaper: Arc<Mutex<Option<Reaper>>>,
}

/// What a child of the job is waited through: it withdraws the child's pid from the reaper before the child is
/// reaped, so a pid the kernel may hand out again is never killed when Flare ends.
#[derive(Clone)]
pub struct Member {
    reaper: Arc<Mutex<Option<Reaper>>>,
}

impl Member {
    /// `Child::try_wait`, withdrawing the pid from the reaper first when the child has exited.
    pub fn try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        if exited_unreaped(child.id(), false) {
            self.release(child.id());
        }
        child.try_wait()
    }

    /// `Child::wait`, withdrawing the pid from the reaper once the child has exited and before it is reaped.
    pub fn wait(&self, child: &mut Child) -> io::Result<ExitStatus> {
        if exited_unreaped(child.id(), true) {
            self.release(child.id());
        }
        child.wait()
    }

    /// Writes `- <pid>` to the reaper, if it holds the pid.
    fn release(&self, pid: u32) {
        let mut guard = lock(&self.reaper);
        let Some(r) = guard.as_mut() else { return };
        if !r.held.contains(&pid) {
            return;
        }
        let _ = r.tell(&format!("- {pid}\n"), 1);
        r.held.retain(|p| *p != pid);
    }
}

/// Whether child `pid` has exited and is still a zombie, read without reaping it (`WNOWAIT`). With `block`, waits
/// for the exit. False for a child already reaped (ECHILD) or still running.
fn exited_unreaped(pid: u32, block: bool) -> bool {
    let raw: libc::id_t = pid;
    let mut opts = libc::WEXITED | libc::WNOWAIT;
    if !block {
        opts |= libc::WNOHANG;
    }
    loop {
        // SAFETY: a zeroed siginfo_t is a valid out value; waitid with WNOWAIT leaves the child waitable.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `info` lives for the call.
        let rc = unsafe { libc::waitid(libc::P_PID, raw, &mut info, opts) };
        if rc == 0 {
            return info.si_pid != 0;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// Makes `cmd` start its child as the leader of a new process group (the group id is the child's pid).
/// Every engine spawn calls it before `spawn`, so the reaper's group kill reaches anything the child starts.
pub fn isolate(cmd: &mut Command) -> &mut Command {
    cmd.process_group(0)
}

/// Whether `pid` names a process that has not been waited on (a zombie still counts: its pid is not free).
fn alive(pid: u32) -> bool {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if raw <= 1 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists and may be signalled; nothing is sent.
    let rc = unsafe { libc::kill(raw, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl Job {
    /// A new job: starts the reaper. `Err` when `/bin/sh` cannot be started.
    pub fn new() -> io::Result<Self> {
        let mut cmd = Command::new(SHELL);
        cmd.args(["-c", REAPER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir("/");
        isolate(&mut cmd);
        // sync-ok: one posix_spawn of /bin/sh when dictation is switched on
        let mut child = cmd.spawn()?;
        let (Some(stdin), Some(out)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the reaper has no pipes"));
        };
        Ok(Job {
            reaper: Arc::new(Mutex::new(Some(Reaper {
                child,
                stdin,
                acks: BufReader::new(out),
                held: Vec::new(),
            }))),
        })
    }

    /// Hands a child to the reaper. `Err` when the job has no running reaper: the caller then kills the child.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        self.assign_pid(child.id())
    }

    fn assign_pid(&self, pid: u32) -> io::Result<()> {
        let mut guard = lock(&self.reaper);
        let Some(r) = guard.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the reaping job has no reaper",
            ));
        };
        if pid <= 1 || pid == std::process::id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a child this job may reap",
            ));
        }
        // A reaper that died (someone killed it) cannot reap anything: refuse, never run unreaped.
        if !matches!(r.child.try_wait(), Ok(None)) {
            return Err(io::Error::other("the reaper is not running"));
        }
        let gone: Vec<u32> = r.held.iter().copied().filter(|p| !alive(*p)).collect();
        let mut lines = String::new();
        for p in &gone {
            lines.push_str(&format!("- {p}\n"));
        }
        lines.push_str(&format!("+ {pid}\n"));
        r.tell(&lines, gone.len() + 1)?;
        r.held.retain(|p| !gone.contains(p));
        if !r.held.contains(&pid) {
            r.held.push(pid);
        }
        Ok(())
    }

    /// What every child handed to this job is waited through (see [`Member`]).
    pub fn member(&self) -> Member {
        Member {
            reaper: self.reaper.clone(),
        }
    }

    /// Whether process `pid` was handed to this job and is still there.
    pub fn contains(&self, pid: u32) -> bool {
        lock(&self.reaper)
            .as_ref()
            .is_some_and(|r| r.held.contains(&pid))
            && alive(pid)
    }

    /// The reaper's own pid, while it runs (tests).
    #[doc(hidden)]
    pub fn reaper_pid(&self) -> Option<u32> {
        lock(&self.reaper).as_ref().map(|r| r.child.id())
    }

    /// A job with no reaper, so every `assign` fails: for tests of the failure path only.
    #[doc(hidden)]
    pub fn unusable_for_tests() -> Self {
        Job {
            reaper: Arc::new(Mutex::new(None)),
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let taken = lock(&self.reaper).take();
        if let Some(Reaper {
            mut child, stdin, ..
        }) = taken
        {
            // End of input: the reaper kills what it holds and exits. Waited for, so no zombie is left and
            // the children are gone when the drop returns.
            drop(stdin);
            let _ = child.wait();
        }
    }
}

/// The one job every dictation child joins for the life of the process (never dropped: the reaper runs until
/// the process ends, and then reaps).
pub fn shared() -> Option<&'static Job> {
    static SHARED: OnceLock<Option<Job>> = OnceLock::new();
    SHARED.get_or_init(|| Job::new().ok()).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// `sleep 60` in its own group, as an engine is started.
    fn sleeper() -> Child {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolate(&mut cmd);
        cmd.spawn().expect("sleep starts")
    }

    fn gone_within(child: &mut Child, d: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < d {
            if child.try_wait().expect("try_wait").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_child_in_the_job_dies_with_the_job() {
        let job = Job::new().expect("a reaper");
        let mut child = sleeper();
        job.assign(&child).expect("the child joins the job");
        assert!(job.contains(child.id()));
        assert!(child.try_wait().expect("try_wait").is_none());
        drop(job);
        // sleep would run 60 s: gone at once is the proof.
        assert!(gone_within(&mut child, Duration::from_secs(10)));
    }

    #[test]
    fn the_reaper_kills_the_childs_whole_group() {
        // A child that starts a grandchild in its group (sh, then sleep): both go.
        let job = Job::new().expect("a reaper");
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "/bin/sleep 60 & echo $! ; wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        isolate(&mut cmd);
        let mut child = cmd.spawn().expect("sh starts");
        job.assign(&child).expect("joins");
        let mut line = String::new();
        let mut out = child.stdout.take().expect("stdout");
        let mut byte = [0u8; 1];
        while std::io::Read::read(&mut out, &mut byte).unwrap_or(0) == 1 && byte[0] != b'\n' {
            line.push(char::from(byte[0]));
        }
        let grand: u32 = line.trim().parse().expect("the grandchild's pid");
        assert!(alive(grand));
        drop(job);
        assert!(gone_within(&mut child, Duration::from_secs(10)));
        let start = Instant::now();
        while alive(grand) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(grand), "the grandchild outlived the job");
    }

    #[test]
    fn the_reaper_reaps_when_its_owner_dies_without_dropping_the_job() {
        // The owner's death is the pipe closing: forgetting the job (no Drop) and closing the write end by
        // hand is what the kernel does to a process that crashed.
        let job = Job::new().expect("a reaper");
        let mut child = sleeper();
        job.assign(&child).expect("joins");
        let reaper = lock(&job.reaper).take().expect("the reaper");
        let Reaper {
            child: mut sh,
            stdin,
            ..
        } = reaper;
        drop(stdin);
        assert!(gone_within(&mut child, Duration::from_secs(10)));
        let _ = sh.wait();
    }

    /// The owner that is killed: run only as the child of the test below (by its variable), it makes a job,
    /// hands it a sleeper, says the sleeper's pid and waits to be killed.
    #[test]
    fn crash_owner_helper() {
        if std::env::var_os("FLARE_JOB_CRASH_OWNER").is_none() {
            return;
        }
        let job = Job::new().expect("a reaper");
        let mut child = sleeper();
        job.assign(&child).expect("joins");
        println!("SLEEPER {}", child.id());
        std::thread::sleep(Duration::from_secs(60));
        drop(job);
        let _ = child.wait();
    }

    #[test]
    fn the_children_die_when_their_owner_is_killed_with_sigkill() {
        // A real crash: the owner never runs a line of its own after the signal, so only the kernel closing
        // its end of the pipe can tell the reaper.
        let mut owner = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", "job::tests::crash_owner_helper", "--nocapture"])
            .env("FLARE_JOB_CRASH_OWNER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the owner starts");
        let out = owner.stdout.take().expect("stdout");
        let mut reader = std::io::BufReader::new(out);
        let mut sleeper_pid = None;
        let mut line = String::new();
        while std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) > 0 {
            if let Some(rest) = line.trim().strip_prefix("SLEEPER ") {
                sleeper_pid = rest.parse::<u32>().ok();
                break;
            }
            line.clear();
        }
        let pid = sleeper_pid.expect("the owner said its sleeper");
        assert!(alive(pid));
        owner.kill().expect("SIGKILL to the owner");
        let _ = owner.wait();
        let start = Instant::now();
        // The sleeper was the owner's child: once killed it is init's, and gone means no pid any more.
        while alive(pid) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "the sleeper outlived its killed owner");
    }

    /// Like `crash_owner_helper`, but a process outside the job inherits the reaper's pipe (the close-on-exec
    /// race): it says both pids and waits to be killed.
    #[test]
    fn leaky_owner_helper() {
        if std::env::var_os("FLARE_JOB_LEAKY_OWNER").is_none() {
            return;
        }
        use std::os::fd::AsRawFd;
        let job = Job::new().expect("a reaper");
        let mut child = sleeper();
        job.assign(&child).expect("joins");
        let fd = lock(&job.reaper)
            .as_ref()
            .expect("reaper")
            .stdin
            .as_raw_fd();
        // SAFETY: dup of a descriptor this process owns; the copy has no close-on-exec, so the next spawn
        // inherits it, as a spawn racing the pipe's creation would.
        let leaked = unsafe { libc::dup(fd) };
        assert!(leaked > 2);
        let mut holder = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the holder starts");
        // SAFETY: the copy made above, closed once.
        unsafe { libc::close(leaked) };
        println!("SLEEPER {} HOLDER {}", child.id(), holder.id());
        std::thread::sleep(Duration::from_secs(60));
        drop(job);
        let _ = child.wait();
        let _ = holder.wait();
    }

    #[test]
    fn the_children_die_with_their_killed_owner_even_when_another_process_holds_the_pipe() {
        let mut owner = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", "job::tests::leaky_owner_helper", "--nocapture"])
            .env("FLARE_JOB_LEAKY_OWNER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the owner starts");
        let out = owner.stdout.take().expect("stdout");
        let mut reader = std::io::BufReader::new(out);
        let mut pids = None;
        let mut line = String::new();
        while std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) > 0 {
            if let Some(rest) = line.trim().strip_prefix("SLEEPER ") {
                let mut it = rest.split(" HOLDER ");
                pids = it
                    .next()
                    .and_then(|a| a.parse::<u32>().ok())
                    .zip(it.next().and_then(|b| b.parse::<u32>().ok()));
                break;
            }
            line.clear();
        }
        let (pid, holder) = pids.expect("the owner said its sleeper and holder");
        assert!(alive(pid) && alive(holder));
        owner.kill().expect("SIGKILL to the owner");
        let _ = owner.wait();
        let start = Instant::now();
        while alive(pid) && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let survived = alive(pid);
        // The holder was never in the job: end it by hand.
        // SAFETY: SIGKILL to the holder the helper started for this test.
        unsafe { libc::kill(libc::pid_t::try_from(holder).unwrap(), libc::SIGKILL) };
        if survived {
            // SAFETY: as above, the sleeper the helper started.
            unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
        }
        assert!(
            !survived,
            "the sleeper outlived its killed owner while the pipe was held open"
        );
    }

    #[test]
    fn a_withdrawn_pid_is_never_killed() {
        // A child that ended and was waited on is withdrawn at the next assign, so a recycled pid is safe.
        let job = Job::new().expect("a reaper");
        let mut first = Command::new("/usr/bin/true").spawn().expect("true starts");
        job.assign(&first).expect("joins");
        let _ = first.wait();
        let mut second = sleeper();
        job.assign(&second).expect("joins");
        assert!(!job.contains(first.id()));
        assert_eq!(
            lock(&job.reaper).as_ref().map(|r| r.held.clone()),
            Some(vec![second.id()])
        );
        drop(job);
        assert!(gone_within(&mut second, Duration::from_secs(10)));
    }

    #[test]
    fn a_child_waited_through_its_member_is_withdrawn_before_it_is_reaped() {
        // Its pid is free once reaped: the reaper must already have let it go (rule 1087), not at the next assign.
        let job = Job::new().expect("a reaper");
        let member = job.member();
        let mut quick = Command::new("/usr/bin/true").spawn().expect("true starts");
        job.assign(&quick).expect("joins");
        let start = Instant::now();
        while member.try_wait(&mut quick).expect("try_wait").is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "true never ended"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let held = || lock(&job.reaper).as_ref().map(|r| r.held.clone());
        assert_eq!(held(), Some(vec![]), "withdrawn with no further assign");
        // The blocking wait too, after a kill.
        let mut slow = sleeper();
        job.assign(&slow).expect("joins");
        slow.kill().expect("kill");
        assert!(member.wait(&mut slow).is_ok());
        assert_eq!(held(), Some(vec![]));
        // The `- <pid>` line reached the reaper: a live child it was told to drop survives the job.
        let mut kept = sleeper();
        job.assign(&kept).expect("joins");
        member.release(kept.id());
        drop(job);
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            kept.try_wait().expect("try_wait").is_none(),
            "a withdrawn pid was killed"
        );
        let _ = kept.kill();
        let _ = kept.wait();
    }

    #[test]
    fn a_job_without_a_running_reaper_refuses() {
        let job = Job::unusable_for_tests();
        let mut child = sleeper();
        assert!(job.assign(&child).is_err());
        assert!(!job.contains(child.id()));
        let _ = child.kill();
        let _ = child.wait();
        // A reaper someone killed: refused too, never a child run outside it.
        let job = Job::new().expect("a reaper");
        let pid = job.reaper_pid().expect("pid");
        // SAFETY: SIGKILL to the reaper this test started.
        unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
        let mut child = sleeper();
        let start = Instant::now();
        let mut refused = false;
        while start.elapsed() < Duration::from_secs(10) {
            if job.assign(&child).is_err() {
                refused = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(refused, "a dead reaper was handed a child");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn the_job_never_takes_itself_or_init() {
        let job = Job::new().expect("a reaper");
        assert!(job.assign_pid(0).is_err());
        assert!(job.assign_pid(1).is_err());
        assert!(job.assign_pid(std::process::id()).is_err());
    }

    #[test]
    fn the_reaper_ignores_lines_that_are_not_a_pid() {
        // Garbage on the pipe must not kill anything or stop the reaper.
        let job = Job::new().expect("a reaper");
        {
            let mut g = lock(&job.reaper);
            let r = g.as_mut().expect("reaper");
            r.stdin
                .write_all(b"+ \n+ -1\n+ abc\nx 5\n- \n")
                .expect("write");
        }
        let mut child = sleeper();
        job.assign(&child).expect("still running, still joins");
        drop(job);
        assert!(gone_within(&mut child, Duration::from_secs(10)));
    }
}
