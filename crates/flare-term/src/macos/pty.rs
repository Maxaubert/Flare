//! macOS half of `pty.rs`: what a tab owns and how closing it ends that.
//!
//! portable-pty starts every shell as a session leader with the pty as its controlling terminal
//! (`setsid`, `TIOCSCTTY`), so the shell's pid is also its session id and its first process group.
//! The tab owns that session: the shell, every job it starts (an interactive shell gives each job a
//! process group of its own, inside the session), and anything they start, even after the starter
//! is gone (an orphan keeps its session). Closing the tab sends SIGHUP to all of it, then SIGTERM and
//! SIGKILL to whatever is left after a short grace: the Windows job object's "closing a tab kills
//! everything under it". Like the GUI-subsystem rule there, an app bundle the shell launched directly
//! (`.app/Contents/MacOS/...`) is spared with everything under it. Quitting waits for the reapers
//! too (`finish_reapers`, which hurries them to SIGTERM and SIGKILL), so a job that ignores SIGHUP
//! does not outlive Flare. A crash of Flare has no job object to lean on: the pty master closes with
//! the process, the hangup ends the shell, and the shell hangs up its jobs; a job that ignores
//! SIGHUP survives a crash (regression rule 232 lists the gaps).

use flare_proc::Signal;
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a process has after SIGHUP before SIGTERM, and after SIGTERM before SIGKILL.
pub(crate) const GRACE: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(500)];

/// One process, as the pure rules read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// The session (its leader's pid); None when it cannot be read.
    pub sid: Option<u32>,
    /// Start time; None when it cannot be read.
    pub start: Option<u64>,
}

/// Pure: the processes that belong to the tab whose shell is `root`, started at `root_start`, the
/// shell itself left out. `gui(pid)` says a process is an app bundle the user launched; it is spared
/// with everything under it.
///
/// Two ways in: a descendant of the shell (followed while the shell is the one the tab started, and
/// only along edges where the child started after its parent, so a recycled parent id leads nowhere),
/// or a member of the shell's session that started after the shell (an orphaned job). When the pid
/// now names ANOTHER process (the shell ended and the number was reused), only members older than
/// that newcomer can be ours: a new session with the same id starts with it.
pub(crate) fn members(
    rows: &[Row],
    root: u32,
    root_start: Option<u64>,
    gui: &dyn Fn(u32) -> bool,
) -> Vec<u32> {
    let leader = rows.iter().find(|r| r.pid == root);
    let ours = leader.is_some_and(|l| root_start.is_some() && l.start == root_start);
    // A newcomer under the root's number: members must be older than it (an unreadable start: none are).
    let cutoff = leader.filter(|_| !ours).map(|l| l.start.unwrap_or(0));
    let mut kids: HashMap<u32, Vec<&Row>> = HashMap::new();
    for r in rows {
        kids.entry(r.ppid).or_default().push(r);
    }
    let mut out: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::from([root]);
    if ours {
        let mut queue = vec![(root, root_start)];
        while let Some((p, p_start)) = queue.pop() {
            for c in kids.get(&p).into_iter().flatten() {
                let born_after = matches!((p_start, c.start), (Some(a), Some(b)) if b >= a);
                if born_after && seen.insert(c.pid) {
                    out.push(c.pid);
                    queue.push((c.pid, c.start));
                }
            }
        }
    }
    for r in rows {
        let after_shell = match (root_start, r.start) {
            (Some(s), Some(t)) => t >= s,
            (None, _) => true,
            (Some(_), None) => false,
        };
        let before_newcomer = cutoff.is_none_or(|c| r.start.is_some_and(|t| t < c));
        if r.sid == Some(root) && after_shell && before_newcomer && seen.insert(r.pid) {
            out.push(r.pid);
        }
    }
    // Spare each app bundle and everything under it.
    let mut spared: HashSet<u32> = HashSet::new();
    for &p in &out {
        if spared.contains(&p) || !gui(p) {
            continue;
        }
        let mut queue = vec![p];
        while let Some(x) = queue.pop() {
            if spared.insert(x) {
                queue.extend(kids.get(&x).into_iter().flatten().map(|r| r.pid));
            }
        }
    }
    out.retain(|p| !spared.contains(p));
    out
}

/// What to signal, pure: a whole process group with one `killpg` when every member of it is a target,
/// each remaining target on its own.
pub(crate) fn plan(rows: &[Row], targets: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let set: HashSet<u32> = targets.iter().copied().collect();
    let mut whole: HashMap<u32, bool> = HashMap::new();
    for r in rows {
        let all = whole.entry(r.pgid).or_insert(true);
        *all &= set.contains(&r.pid);
    }
    let mut groups: Vec<u32> = whole
        .into_iter()
        .filter(|&(g, all)| all && g > 1 && set.contains(&g))
        .map(|(g, _)| g)
        .collect();
    groups.sort_unstable();
    let in_group: HashSet<u32> = groups.iter().copied().collect();
    let singles = rows
        .iter()
        .filter(|r| set.contains(&r.pid) && !in_group.contains(&r.pgid))
        .map(|r| r.pid)
        .collect();
    (groups, singles)
}

/// Pure: the processes an earlier round found to be the tab's that still run as the same process
/// (same pid AND start: a recycled number is a stranger) and are not targets already.
pub(crate) fn remembered(rows: &[Row], seen: &HashSet<(u32, u64)>, targets: &[u32]) -> Vec<u32> {
    rows.iter()
        .filter(|r| r.start.is_some_and(|s| seen.contains(&(r.pid, s))))
        .filter(|r| !targets.contains(&r.pid))
        .map(|r| r.pid)
        .collect()
}

/// The process table the rules read, now.
fn rows_now() -> Vec<Row> {
    flare_proc::table()
        .into_iter()
        .map(|s| Row {
            pid: s.pid,
            ppid: s.ppid,
            pgid: s.pgid,
            sid: flare_proc::session_of(s.pid),
            start: s.start_us,
        })
        .collect()
}

/// An app bundle's own executable: the macOS counterpart of a GUI-subsystem image.
fn is_app_bundle(pid: u32) -> bool {
    flare_proc::exe(pid).is_some_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
}

/// The session a tab owns, by its shell's pid and start time.
pub(crate) struct Tab {
    inner: Arc<TabInner>,
}

struct TabInner {
    root: u32,
    start: Option<u64>,
    /// A duplicate of the pty master, for `discard_output` while the session dies. Closed when the
    /// pty and every reaper are done with it.
    master: Option<OwnedFd>,
    /// A reaper thread is escalating; a second kill only repeats the hangup.
    reaping: AtomicBool,
    /// Every (pid, start) a round found to be the tab's: one that leaves the session and the tree
    /// later (its parent exits after it called `setsid`) is still ended while it is that process.
    seen: Mutex<HashSet<(u32, u64)>>,
}

/// How many reaper threads are running, in every tab, and a wake for `finish_reapers`.
static REAPERS: Mutex<usize> = Mutex::new(0);
static REAPERS_CV: Condvar = Condvar::new();
/// Set while the quit waits: the reapers skip the rest of their grace.
static HURRY: AtomicBool = AtomicBool::new(false);

/// BLOCKING, at most `timeout`: the quit's wait for every reaper. The reapers skip what is left of
/// their grace (SIGTERM at their next look, SIGKILL at the one after), so a job that ignores SIGHUP
/// is ended before Flare exits rather than outliving it. True when no reaper is left.
pub(crate) fn finish_reapers(timeout: Duration) -> bool {
    HURRY.store(true, Ordering::SeqCst);
    let n = REAPERS.lock().unwrap_or_else(|p| p.into_inner());
    let (n, _) = REAPERS_CV
        .wait_timeout_while(n, timeout, |n| *n > 0) // sync-ok: the quit path waits on purpose, with a cap
        .unwrap_or_else(|p| p.into_inner());
    let done = *n == 0;
    drop(n);
    HURRY.store(false, Ordering::SeqCst);
    done
}

fn reapers_add(d: isize) {
    let mut n = REAPERS.lock().unwrap_or_else(|p| p.into_inner());
    *n = n.saturating_add_signed(d);
    REAPERS_CV.notify_all();
}

impl Tab {
    /// The tab of the shell `pid` in the pty `master`, read right after the spawn (the shell is the
    /// session leader already).
    pub(crate) fn of(pid: u32, master: Option<RawFd>) -> Tab {
        Tab {
            inner: Arc::new(TabInner {
                root: pid,
                start: flare_proc::stat(pid).and_then(|s| s.start_us),
                master: master.and_then(flare_proc::dup_fd),
                reaping: AtomicBool::new(false),
                seen: Mutex::new(HashSet::new()),
            }),
        }
    }

    /// SIGHUP to the whole session now; SIGTERM, then SIGKILL, to what is left, on a reaper thread
    /// that also throws away the unread output a dying process would wait on. Never blocks (one
    /// table read). Safe to call again.
    pub(crate) fn end(&self) {
        let t = &self.inner;
        if t.root == 0 {
            return;
        }
        t.discard();
        if t.step(Some(Signal::Hup)) == 0 || t.reaping.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        // Counted before the thread starts, so a quit that waits right after the kill sees it.
        reapers_add(1);
        let spawned = std::thread::Builder::new()
            .name("flare-pty-reap".into())
            .spawn(move || {
                inner.reap();
                inner.reaping.store(false, Ordering::SeqCst);
                reapers_add(-1);
            });
        if spawned.is_err() {
            reapers_add(-1);
            // No thread: no grace either.
            t.step(Some(Signal::Kill));
            t.discard();
            t.reaping.store(false, Ordering::SeqCst);
        }
    }
}

impl TabInner {
    fn discard(&self) {
        if let Some(fd) = &self.master {
            flare_proc::discard_output(fd.as_fd());
        }
    }

    /// One round: `sig` (or nothing, to count) to the shell while it is the tab's, and to every
    /// member. The number of processes still the tab's.
    fn step(&self, sig: Option<Signal>) -> usize {
        let rows = rows_now();
        let mut targets = members(&rows, self.root, self.start, &is_app_bundle);
        {
            let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            for r in &rows {
                if let Some(start) = r.start
                    && targets.contains(&r.pid)
                {
                    seen.insert((r.pid, start));
                }
            }
            for p in remembered(&rows, &seen, &targets) {
                if !is_app_bundle(p) {
                    targets.push(p);
                }
            }
        }
        if self.start.is_some()
            && rows
                .iter()
                .any(|r| r.pid == self.root && r.start == self.start)
        {
            targets.push(self.root);
        }
        if let Some(sig) = sig {
            let (groups, singles) = plan(&rows, &targets);
            for g in groups {
                flare_proc::signal_group(g, sig);
            }
            for p in singles {
                flare_proc::signal(p, sig);
            }
        }
        targets.len()
    }

    /// The reaper: every tick throws away unread output and counts what is left; SIGTERM after the
    /// first grace, SIGKILL after the second. Done when nothing of the tab is left, or at REAP_CAP.
    fn reap(&self) {
        let t0 = Instant::now();
        let mut due = vec![
            (GRACE[0] + GRACE[1], Signal::Kill),
            (GRACE[0], Signal::Term),
        ];
        loop {
            std::thread::sleep(TICK); // sync-ok: the reaper's own thread, between two looks
            self.discard();
            let hurry = HURRY.load(Ordering::SeqCst);
            let sig = match due.last() {
                Some(&(at, sig)) if hurry || t0.elapsed() >= at => {
                    due.pop();
                    Some(sig)
                }
                _ => None,
            };
            if self.step(sig) == 0 || t0.elapsed() >= REAP_CAP {
                return;
            }
        }
    }
}

/// How often the reaper looks.
const TICK: Duration = Duration::from_millis(40);
/// The reaper gives up after this: a process SIGKILL cannot end (stuck in the kernel) is left.
const REAP_CAP: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, pgid: u32, sid: u32, start: u64) -> Row {
        Row {
            pid,
            ppid,
            pgid,
            sid: Some(sid),
            start: Some(start),
        }
    }

    fn sorted(mut v: Vec<u32>) -> Vec<u32> {
        v.sort_unstable();
        v
    }

    #[test]
    fn the_session_jobs_and_their_children_belong_to_the_tab() {
        // shell 10 (session 10); job 20 in its own group; 21 its child; 30 a setsid daemon the job
        // started (another session, still a descendant); 40 an orphan of the session (parent launchd).
        let rows = [
            row(10, 5, 10, 10, 100),
            row(20, 10, 20, 10, 110),
            row(21, 20, 20, 10, 111),
            row(30, 20, 30, 30, 112),
            row(40, 1, 40, 10, 120),
            row(50, 1, 50, 50, 90),
        ];
        let m = members(&rows, 10, Some(100), &|_| false);
        assert_eq!(sorted(m), [20, 21, 30, 40]);
    }

    #[test]
    fn an_app_bundle_and_everything_under_it_is_spared() {
        let rows = [
            row(10, 5, 10, 10, 100),
            row(20, 10, 20, 10, 110),
            row(21, 20, 20, 10, 111),
            row(30, 10, 30, 10, 115),
        ];
        let m = members(&rows, 10, Some(100), &|p| p == 20);
        assert_eq!(m, [30]);
    }

    #[test]
    fn a_child_older_than_its_listed_parent_is_a_recycled_number() {
        // 20 lists the shell as parent but started before it, in another session: not ours.
        let rows = [row(10, 5, 10, 10, 100), row(20, 10, 20, 20, 50)];
        assert!(members(&rows, 10, Some(100), &|_| false).is_empty());
        // An unreadable start proves nothing: spared.
        let mut r = row(30, 10, 30, 30, 0);
        r.start = None;
        assert!(members(&[rows[0], r], 10, Some(100), &|_| false).is_empty());
    }

    #[test]
    fn after_the_shell_exits_its_orphans_are_still_the_tabs() {
        // The shell 10 is gone; its background job 40 lives on in session 10.
        let rows = [row(40, 1, 40, 10, 120), row(41, 40, 40, 10, 121)];
        assert_eq!(sorted(members(&rows, 10, Some(100), &|_| false)), [40, 41]);
    }

    #[test]
    fn a_reused_shell_pid_is_never_mistaken_for_the_tab() {
        // The shell 10 ended; the number now leads another tab's session (started at 500), whose
        // processes (600) must be left alone, while the old orphan (120) is still ours.
        let rows = [
            row(10, 5, 10, 10, 500),
            row(11, 10, 11, 10, 600),
            row(40, 1, 40, 10, 120),
        ];
        assert_eq!(members(&rows, 10, Some(100), &|_| false), [40]);
        // A newcomer whose start cannot be read: nothing of the session is touched.
        let mut lead = rows[0];
        lead.start = None;
        assert!(members(&[lead, rows[1], rows[2]], 10, Some(100), &|_| false).is_empty());
    }

    #[test]
    fn a_member_seen_once_is_followed_out_of_the_session_but_never_a_recycled_number() {
        // 30 was a member in an earlier round; its parent exited after it called setsid, so it now
        // hangs under launchd in a session of its own. 31 reused an old member's number.
        let rows = [
            row(30, 1, 30, 30, 112),
            row(31, 1, 31, 31, 900),
            row(40, 1, 40, 10, 120),
        ];
        let seen = HashSet::from([(30, 112), (31, 113), (40, 120)]);
        let now = members(&rows, 10, Some(100), &|_| false);
        assert_eq!(now, [40], "only the orphan is still in the session");
        assert_eq!(remembered(&rows, &seen, &now), [30]);
    }

    #[test]
    fn a_whole_group_gets_one_killpg_and_a_shared_group_gets_single_signals() {
        // Group 20 is all targets; group 30 also holds 32, which is spared.
        let rows = [
            row(20, 10, 20, 10, 1),
            row(21, 20, 20, 10, 1),
            row(30, 10, 30, 10, 1),
            row(31, 30, 30, 10, 1),
            row(32, 30, 30, 10, 1),
            row(40, 1, 7, 10, 1),
        ];
        let (groups, mut singles) = plan(&rows, &[20, 21, 30, 31, 40]);
        singles.sort_unstable();
        assert_eq!(groups, [20]);
        // 40's group 7 is not a target's own group: it is signalled alone.
        assert_eq!(singles, [30, 31, 40]);
        // launchd's group 1 is never signalled whole.
        let (groups, singles) = plan(&[row(9, 1, 1, 1, 1)], &[9]);
        assert!(groups.is_empty());
        assert_eq!(singles, [9]);
    }

    /// Real shells in a real pty: spawn, talk, resize, the foreground job and its folder, and the kill.
    mod real {
        use crate::pty::{PortableSpawner, PtyProcess, PtySpawner, SpawnSpec};
        use std::io::{Read, Write};
        use std::path::{Path, PathBuf};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        type Shared = Arc<Mutex<Vec<u8>>>;

        fn env() -> Vec<(String, String)> {
            vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("TERM".into(), "dumb".into()),
                ("PS1".into(), "flare$ ".into()),
                ("FLARE_T".into(), "1".into()),
            ]
        }

        fn spec(args: &[&str], cwd: &Path) -> SpawnSpec {
            SpawnSpec {
                exe: "/bin/sh".into(),
                args: args.iter().map(|s| s.to_string()).collect(),
                cwd: cwd.to_string_lossy().into_owned(),
                env: env(),
                cols: 80,
                rows: 24,
            }
        }

        struct Shell {
            p: Box<dyn PtyProcess>,
            out: Shared,
            w: Box<dyn Write + Send>,
        }

        impl Shell {
            fn start(args: &[&str], cwd: &Path) -> Shell {
                let mut p = PortableSpawner.spawn(&spec(args, cwd)).unwrap();
                let mut r = p.take_reader().unwrap();
                let w = p.take_writer().unwrap();
                let out: Shared = Arc::new(Mutex::new(Vec::new()));
                let o = Arc::clone(&out);
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 8192];
                    while let Ok(n) = r.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        o.lock().unwrap().extend_from_slice(&chunk[..n]);
                    }
                });
                Shell { p, out, w }
            }

            fn interactive(cwd: &Path) -> Shell {
                let s = Shell::start(&["-i"], cwd);
                assert!(s.saw("flare$ "), "no prompt: {}", s.text());
                s
            }

            fn text(&self) -> String {
                String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
            }

            fn saw(&self, needle: &str) -> bool {
                until(10_000, || self.text().contains(needle))
            }

            fn say(&mut self, line: &str) {
                self.w.write_all(line.as_bytes()).unwrap();
                self.w.write_all(b"\n").unwrap();
                self.w.flush().unwrap();
            }
        }

        impl Drop for Shell {
            fn drop(&mut self) {
                self.p.kill();
            }
        }

        fn until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
            let end = Instant::now() + Duration::from_millis(ms);
            loop {
                if f() {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn canon(p: &Path) -> PathBuf {
            p.canonicalize().unwrap()
        }

        /// The processes of the session `sid` whose argv is `sleep <secs>`.
        fn sleeps_in(sid: u32, secs: &str) -> Vec<u32> {
            flare_proc::table()
                .into_iter()
                .filter(|r| flare_proc::session_of(r.pid) == Some(sid))
                .filter(|r| {
                    flare_proc::argv(r.pid)
                        .is_some_and(|a| a.len() == 2 && a[0].ends_with("sleep") && a[1] == secs)
                })
                .map(|r| r.pid)
                .collect()
        }

        #[test]
        fn sh_starts_answers_and_its_environment_is_exactly_what_was_given() {
            let dir = tempfile::tempdir().unwrap();
            let mut s = Shell::interactive(dir.path());
            assert!(s.p.pid() > 0);
            s.say("echo flare-$((40+2))");
            assert!(s.saw("flare-42"), "{}", s.text());
            let e = Shell::start(&["-c", "/usr/bin/env; echo env-done"], dir.path());
            assert!(e.saw("env-done"), "{}", e.text());
            let t = e.text();
            assert!(t.contains("FLARE_T=1"), "{t}");
            // Nothing of this process's own environment: only SHELL is added (portable-pty names the
            // user's login shell when the spec does not), and the shell's own PWD, SHLVL and _.
            assert!(!t.contains("HOME="), "the parent's HOME leaked in: {t}");
            assert!(!t.contains("USER="), "the parent's USER leaked in: {t}");
        }

        #[test]
        fn an_exit_is_reported_with_its_code() {
            let dir = tempfile::tempdir().unwrap();
            let mut s = Shell::interactive(dir.path());
            let (tx, rx) = std::sync::mpsc::channel();
            s.p.on_exit(Box::new(move |c| {
                let _ = tx.send(c);
            }));
            s.say("exit 3");
            assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Some(3));
            assert_eq!(s.p.exit_code(), Some(3));
            // After the exit, a kill finds nothing to end and does not stall.
            let t = Instant::now();
            s.p.kill();
            assert!(t.elapsed() < Duration::from_millis(500));
        }

        #[test]
        fn a_resize_reaches_the_shell() {
            let dir = tempfile::tempdir().unwrap();
            let mut s = Shell::interactive(dir.path());
            s.p.resize(100, 30).unwrap();
            s.say("stty size");
            assert!(s.saw("30 100"), "{}", s.text());
        }

        #[test]
        fn the_foreground_job_and_its_folder_follow_a_cd() {
            let dir = tempfile::tempdir().unwrap();
            let sub = dir.path().join("deeper");
            std::fs::create_dir(&sub).unwrap();
            let mut s = Shell::interactive(dir.path());
            let shell = s.p.pid();
            assert_eq!(s.p.foreground_pid(), Some(shell), "the shell at its prompt");
            assert_eq!(canon(&flare_proc::cwd(shell).unwrap()), canon(dir.path()));
            s.say("cd deeper && /bin/sleep 30");
            assert!(
                until(10_000, || s.p.foreground_pid().is_some_and(|p| p != shell)),
                "no job came to the foreground"
            );
            let job = s.p.foreground_pid().unwrap();
            assert_eq!(flare_proc::stat(job).unwrap().ppid, shell);
            assert_eq!(flare_proc::argv(job).unwrap(), ["/bin/sleep", "30"]);
            assert_eq!(canon(&flare_proc::cwd(job).unwrap()), canon(&sub));
            assert_eq!(canon(&flare_proc::cwd(shell).unwrap()), canon(&sub));
        }

        #[test]
        fn closing_the_tab_ends_the_shell_its_jobs_orphans_and_stubborn_children() {
            let dir = tempfile::tempdir().unwrap();
            let mut s = Shell::interactive(dir.path());
            let shell = s.p.pid();
            // A plain job, an orphan (its subshell is gone), one that ignores SIGHUP, one that also
            // ignores SIGTERM, and a foreground job.
            s.say("/bin/sleep 301 & (/bin/sleep 302 &); /bin/sh -c 'trap \"\" HUP; /bin/sleep 303' & /bin/sh -c 'trap \"\" HUP TERM; /bin/sleep 304; :' & /bin/sleep 305");
            let tags = ["301", "302", "303", "304", "305"];
            assert!(
                until(10_000, || tags
                    .iter()
                    .all(|t| !sleeps_in(shell, t).is_empty())),
                "not every child started: {}",
                s.text()
            );
            let mut all: Vec<u32> = tags.iter().flat_map(|t| sleeps_in(shell, t)).collect();
            all.extend(
                flare_proc::table()
                    .into_iter()
                    .filter(|r| flare_proc::session_of(r.pid) == Some(shell))
                    .map(|r| r.pid),
            );
            all.sort_unstable();
            all.dedup();
            assert!(all.contains(&shell));
            let t = Instant::now();
            s.p.kill();
            assert!(t.elapsed() < Duration::from_millis(200), "kill blocked");
            assert!(
                until(5_000, || all.iter().all(|p| !flare_proc::alive(*p))),
                "still alive: {:?}",
                all.iter()
                    .filter(|p| flare_proc::alive(**p))
                    .map(|p| (p, flare_proc::argv(*p)))
                    .collect::<Vec<_>>()
            );
            assert!(until(5_000, || s.p.exit_code().is_some()));
        }

        /// Helper half: only runs when the parent test sets the variable. Starts a shell with a job
        /// and a foreground child, writes their pids, then dies without any destructor (a crash).
        #[test]
        fn crash_helper_starts_a_shell_then_aborts() {
            let Ok(out) = std::env::var("FLARE_MAC_CRASH_HELPER_OUT") else {
                return;
            };
            let dir = std::env::temp_dir();
            let mut s = Shell::interactive(&dir);
            let shell = s.p.pid();
            s.say("/bin/sleep 311 & /bin/sleep 312");
            assert!(until(10_000, || {
                !sleeps_in(shell, "311").is_empty() && !sleeps_in(shell, "312").is_empty()
            }));
            let mut pids = vec![shell];
            pids.extend(sleeps_in(shell, "311"));
            pids.extend(sleeps_in(shell, "312"));
            let text: Vec<String> = pids.iter().map(u32::to_string).collect();
            std::fs::write(&out, text.join(" ")).unwrap();
            std::mem::forget(s);
            std::process::abort();
        }

        #[test]
        fn a_crash_of_the_terminal_hangs_up_its_shells() {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("pids.txt");
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pty::macos::tests::real::crash_helper_starts_a_shell_then_aborts",
                    "--nocapture",
                ])
                .env("FLARE_MAC_CRASH_HELPER_OUT", &out)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "the helper aborts");
            let pids: Vec<u32> = std::fs::read_to_string(&out)
                .expect("helper wrote its pids")
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(pids.len(), 3, "{pids:?}");
            let gone = until(10_000, || pids.iter().all(|p| !flare_proc::alive(*p)));
            let left: Vec<u32> = pids
                .iter()
                .copied()
                .filter(|p| flare_proc::alive(*p))
                .collect();
            for p in &left {
                // Do not leave the orphans this very test found.
                flare_proc::signal(*p, flare_proc::Signal::Kill);
            }
            assert!(gone, "outlived the crash: {left:?}");
        }
    }
}
