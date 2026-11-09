//! Process control on macOS (rule 335): the Mac meaning of `proc.rs`. A launched app gets its own
//! process group, and dropping it kills that group and every descendant (the Windows job object's
//! kill-on-close; a shell the app starts sits in its own session, so the tree is walked by parent pid).
//! Reaping is by profile path only, as on Windows: a process whose command line does not hold the world's
//! path is never touched, so the owner's own Flare is safe.

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Windows `CommandExt::creation_flags` as a no-op, so a scenario that hides a console window
/// on Windows compiles unchanged on macOS (a Mac child never makes a window of its own).
pub trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
}

impl CommandExt for Command {
    fn creation_flags(&mut self, _flags: u32) -> &mut Self {
        self
    }
}

/// The argv a launch passes: the caller's, plus `--e2e` and `--user-data-dir=<profile>` unless
/// the caller gave its own.
pub fn launch_args(args: &[&str], profile: &Path) -> Vec<String> {
    let mut v: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    if !v.iter().any(|a| a == "--e2e") {
        v.push("--e2e".into());
    }
    if !v.iter().any(|a| a.starts_with("--user-data-dir")) {
        v.push(format!("--user-data-dir={}", profile.display()));
    }
    v
}

/// One launched app. Dropping it kills its process group and its descendants, then reaps by profile.
pub struct Child {
    proc: Mutex<std::process::Child>,
    profile: PathBuf,
}

impl Child {
    /// Starts `exe` with `launch_args`, the environment on top of the harness's own, in its own
    /// process group.
    pub fn launch(exe: &Path, args: &[&str], env: &[(String, String)], profile: &Path) -> Child {
        let mut cmd = Command::new(exe);
        cmd.args(launch_args(args, profile));
        for (k, v) in env {
            cmd.env(k, v);
        }
        Self::spawn(cmd, profile)
    }

    /// Spawns `cmd` as the leader of a new process group (so a kill reaches what it starts in it).
    pub fn spawn(mut cmd: Command, profile: &Path) -> Child {
        cmd.process_group(0);
        let proc = cmd.spawn().expect("spawn");
        Child {
            proc: Mutex::new(proc),
            profile: profile.to_path_buf(),
        }
    }

    /// Polls `what` every 25 ms until it holds or `timeout` passes.
    pub fn wait_for(&self, what: impl Fn() -> bool, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if what() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25)); // sync-ok: test harness, off the app
        }
    }

    /// The exit code once the child has exited (-1 for a signal).
    pub fn exited(&self) -> Option<i32> {
        let mut p = self.proc.lock().unwrap_or_else(|e| e.into_inner());
        p.try_wait().ok().flatten().map(|s| s.code().unwrap_or(-1))
    }

    /// macOS has no job object: the child's tree is its process group and its descendants, and the
    /// child is in it while it runs.
    pub fn in_job(&self) -> bool {
        self.exited().is_none()
    }

    pub fn pid(&self) -> u32 {
        self.proc.lock().unwrap_or_else(|e| e.into_inner()).id()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        let pid = self.pid();
        kill_tree(pid);
        {
            let mut p = self.proc.lock().unwrap_or_else(|e| e.into_inner());
            let _ = p.kill();
            let _ = p.wait();
        }
        reap_by_profile(&self.profile);
    }
}

/// Every process as (pid, parent pid, command line), read from `ps` (no special right needed).
pub fn process_rows() -> Vec<(u32, u32, String)> {
    let Ok(out) = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,ppid=,command="])
        .output()
    else {
        return Vec::new();
    };
    parse_ps(&String::from_utf8_lossy(&out.stdout))
}

/// Pure: the rows of `ps -o pid=,ppid=,command=`.
pub fn parse_ps(text: &str) -> Vec<(u32, u32, String)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let rest: Vec<&str> = it.collect();
            Some((pid, ppid, rest.join(" ")))
        })
        .collect()
}

/// Pure: `root` and every process below it in `rows`.
pub fn tree_of(root: u32, rows: &[(u32, u32, String)]) -> Vec<u32> {
    let mut set = vec![root];
    loop {
        let before = set.len();
        for (pid, ppid, _) in rows {
            if set.contains(ppid) && !set.contains(pid) {
                set.push(*pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// Kills `root`'s process group and every descendant. Returns how many signals were sent.
fn kill_tree(root: u32) -> usize {
    if root == 0 || root == std::process::id() {
        return 0;
    }
    let rows = process_rows();
    let tree = tree_of(root, &rows);
    let mut sent = 0;
    // SAFETY: plain signal calls on pids read just now; a gone pid is an error, not a fault.
    unsafe {
        if libc::killpg(root as libc::pid_t, libc::SIGKILL) == 0 {
            sent += 1;
        }
    }
    for pid in tree {
        if pid != std::process::id() {
            // SAFETY: as above.
            if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0 {
                sent += 1;
            }
        }
    }
    sent
}

/// Pure: whether a command line holds the world's path (case-insensitive, as the Mac file system).
pub fn holds_profile(cmdline: &str, profile: &str) -> bool {
    profile.len() >= 4 && cmdline.to_lowercase().contains(&profile.to_lowercase())
}

/// Kills, with their trees, every process (not this one) whose command line holds `profile`.
/// Returns how many it killed.
pub fn reap_by_profile(profile: &Path) -> usize {
    let needle = profile.to_string_lossy().into_owned();
    if needle.len() < 4 {
        return 0;
    }
    let me = std::process::id();
    let rows = process_rows();
    let mut killed = 0;
    for (pid, _, cmd) in &rows {
        if *pid == me || *pid == 0 || !holds_profile(cmd, &needle) {
            continue;
        }
        for p in tree_of(*pid, &rows) {
            // SAFETY: a plain signal on a pid just read; a gone pid is an error, not a fault.
            if p != me && unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) } == 0 {
                killed += 1;
            }
        }
    }
    killed
}

/// How many on-screen windows the processes `pids` own (the window server's list, which needs no
/// Screen Recording right for the owner pid and the on-screen flag).
pub fn visible_top_windows_of(pids: &[u32]) -> usize {
    crate::mac::on_screen_windows_of(pids)
}

/// Whether `pid` is a live process.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the pid.
    pid != 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_rows_parse_and_a_tree_holds_every_descendant() {
        let rows = parse_ps("  10     1 /a/flare --e2e --user-data-dir=/t/flare-e2e-x/p\n 11 10 /bin/zsh -l\n 12 11 sleep 9\n 13 1 other\nbad\n");
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1], (11, 10, "/bin/zsh -l".to_string()));
        let mut t = tree_of(10, &rows);
        t.sort_unstable();
        assert_eq!(t, vec![10, 11, 12]);
    }

    #[test]
    fn the_reaper_spares_a_process_without_the_world_path() {
        assert!(holds_profile(
            "/a/flare --user-data-dir=/T/Flare-E2E-x/p",
            "/t/flare-e2e-x"
        ));
        assert!(!holds_profile(
            "/Applications/Flare.app/Contents/MacOS/flare",
            "/t/flare-e2e-x"
        ));
        assert!(
            !holds_profile("anything", "/t"),
            "a short path never matches"
        );
    }

    #[test]
    fn a_launched_tree_is_killed_when_the_child_drops() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & sleep 30; wait"]);
        let child = Child::spawn(cmd, dir.path());
        let pid = child.pid();
        assert!(child.wait_for(
            || tree_of(pid, &process_rows()).len() >= 2,
            Duration::from_secs(5)
        ));
        let tree = tree_of(pid, &process_rows());
        drop(child);
        let gone = || tree.iter().all(|p| !pid_alive(*p) || zombie(*p));
        let end = Instant::now() + Duration::from_secs(5);
        while !gone() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(gone(), "every process of the tree is gone: {tree:?}");
    }

    fn zombie(pid: u32) -> bool {
        Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
            .unwrap_or(true)
    }
}
