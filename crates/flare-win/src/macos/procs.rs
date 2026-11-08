//! The agent poll's process snapshot on macOS: libproc for every process with its parent and start,
//! sysctl `KERN_PROCARGS2` for the argv of the descendants of the live shells, kqueue for the exit
//! wait (all through `flare-proc`).

use super::{CMD_CAP, ProcEntry, born_after, descendants_where};
use std::collections::{HashMap, HashSet};

/// Pure: an argv as one command line, the way a Windows command line reads: arguments joined by
/// spaces, one that is empty or holds a space, a tab or a quote wrapped in double quotes (inner
/// quotes and the backslashes before them escaped). Cut at CMD_CAP chars.
pub fn join_argv(argv: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            out.push_str(a);
            continue;
        }
        out.push('"');
        let mut slashes = 0usize;
        for c in a.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    slashes = 0;
                    out.push('"');
                    continue;
                }
                _ => {}
            }
            if c != '\\' {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
    }
    match out.char_indices().nth(CMD_CAP) {
        Some((at, _)) => out[..at].to_string(),
        None => out,
    }
}

/// BLOCKING (the poll's worker). Every process as (pid, ppid); `cmd` is read ONLY for `roots` and their
/// descendants and kept only when `wanted(&cmd)` says so. Unlike Windows the roots' own command lines
/// are read: a shell that ran `exec claude` IS the agent, with the shell's pid
/// (`flare_agent::detect::tab_agent_kind`). None when the table cannot be read.
pub fn snapshot_processes(roots: &[u32], wanted: &dyn Fn(&str) -> bool) -> Option<Vec<ProcEntry>> {
    let table = flare_proc::table();
    if table.is_empty() {
        return None;
    }
    let start: HashMap<u32, Option<u64>> = table.iter().map(|r| (r.pid, r.start_us)).collect();
    let rows: Vec<(u32, u32)> = table.iter().map(|r| (r.pid, r.ppid)).collect();
    let want: HashSet<u32> = descendants_where(&rows, roots, &|parent, child| {
        born_after(
            start.get(&parent).copied().flatten(),
            start.get(&child).copied().flatten(),
        )
    })
    .into_iter()
    .chain(roots.iter().copied().filter(|r| *r != 0))
    .collect();
    Some(
        rows.into_iter()
            .map(|(pid, ppid)| {
                let cmd = if want.contains(&pid) {
                    let c = flare_proc::argv(pid)
                        .map(|a| join_argv(&a))
                        .unwrap_or_default();
                    if wanted(&c) { c } else { String::new() }
                } else {
                    String::new()
                };
                ProcEntry { pid, ppid, cmd }
            })
            .collect(),
    )
}

/// Waits until process `pid` has ended, at most `max`. True when it is gone. A kqueue `NOTE_EXIT`
/// watch (a zombie counts as gone).
pub fn wait_for_exit(pid: u32, max: std::time::Duration) -> bool {
    flare_proc::wait_for_exit(pid, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_argv_reads_as_a_command_line() {
        assert_eq!(
            join_argv(&v(&[
                "node",
                "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
                "--resume"
            ])),
            "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js --resume"
        );
        assert_eq!(
            join_argv(&v(&["/bin/sh", "-c", "echo hi", ""])),
            r#"/bin/sh -c "echo hi" """#
        );
        assert_eq!(join_argv(&v(&["a\"b"])), r#""a\"b""#);
        assert_eq!(join_argv(&v(&["x y\\"])), r#""x y\\""#);
        assert_eq!(join_argv(&v(&["p\\q r"])), r#""p\q r""#);
        assert_eq!(join_argv(&[]), "");
        let big = vec!["a".repeat(40_000)];
        assert_eq!(join_argv(&big).chars().count(), CMD_CAP);
    }

    #[test]
    fn a_real_child_command_line_is_read_and_filtered() {
        use std::process::{Command, Stdio};
        // sh waits for its sleep, so the sleep is a grandchild of this process.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 8; : claude-code-test"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sh"); // sync-ok: test code on the test thread only
        let me = child.id();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let rows = loop {
            let rows = snapshot_processes(&[std::process::id()], &|c| {
                c.to_lowercase().contains("claude")
                    || c.contains("sleep")
                    || c.contains("flare_win")
            })
            .expect("snapshot");
            if rows.iter().any(|r| r.ppid == me) || std::time::Instant::now() > end {
                break rows;
            }
            std::thread::sleep(std::time::Duration::from_millis(20)); // sync-ok: test code on the test thread only
        };
        let _ = Command::new("/usr/bin/pkill")
            .args(["-P", &me.to_string()])
            .status(); // sync-ok: test code on the test thread only
        let _ = child.kill();
        let _ = child.wait(); // sync-ok: test code on the test thread only
        // The root's own command line is read too (a shell that became the agent by `exec`).
        let own = rows
            .iter()
            .find(|r| r.pid == std::process::id())
            .expect("self");
        assert!(!own.cmd.is_empty(), "the root's own command line is read");
        let c = rows.iter().find(|r| r.pid == me).expect("child present");
        assert!(c.cmd.contains("claude-code-test"), "cmd was {:?}", c.cmd);
        assert!(c.cmd.starts_with("/bin/sh -c "), "cmd was {:?}", c.cmd);
        let grand = rows
            .iter()
            .find(|r| r.ppid == me)
            .expect("sleep grandchild present");
        assert_eq!(grand.cmd, "/bin/sleep 8");
        // A process outside the roots' trees never has its command line read.
        let launchd = rows.iter().find(|r| r.pid == 1).expect("launchd");
        assert!(launchd.cmd.is_empty());
    }

    #[test]
    fn the_snapshot_includes_every_process_even_without_roots() {
        let rows = snapshot_processes(&[], &|_| true).expect("snapshot");
        let me = rows
            .iter()
            .find(|r| r.pid == std::process::id())
            .expect("self");
        assert!(me.ppid != 0);
        assert!(rows.iter().all(|r| r.cmd.is_empty()));
    }

    #[test]
    fn an_unreadable_process_is_not_an_error() {
        // launchd (1) is root's: its argv cannot be read, and that is no failure.
        let rows = snapshot_processes(&[1, 0], &|_| true).expect("snapshot");
        assert!(rows.iter().any(|r| r.pid == 1));
    }
}
