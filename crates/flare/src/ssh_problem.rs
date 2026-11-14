//! What ssh said when a link did not come up, in plain words (machine pairing PR 1, rule 1250).
//!
//! ssh's stderr is no longer thrown away: a link (`remote_link::start_link`) and a Preview tunnel
//! (`remote_tunnel::open_ssh`) keep its last lines ([`StderrTail`]), and a failed link carries them after
//! [`SAID`] in its message. [`sort_down`] reads only that part (Flare's own words never match by chance) and sorts
//! it into a few cases, each one short line: the grey line under a machine's row and the red line in the Add a machine
//! dialog. Anything else keeps the old line ("No Flare answered over SSH on <host>.").

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

/// Where ssh's own words start in a failed link's message.
pub const SAID: &str = "ssh said: ";

/// The mark a link that could not start ssh at all carries (no ssh client on this machine).
pub const NO_CLIENT: &str = "no ssh client: ";

/// How many of ssh's last lines are kept, and how long each may be.
pub const TAIL_LINES: usize = 12;
pub const LINE_MAX: usize = 400;

/// What went wrong, as far as ssh's words tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshProblem {
    /// Refused, timed out, no route: nothing answers on ssh's port.
    NotAnswering,
    /// The host refused this computer's key.
    NotPaired,
    /// The host's key is not the one this computer trusts (changed, or none known with strict checking).
    IdentityChanged,
    /// The name could not be looked up.
    NotFound,
    /// This machine has no ssh client.
    NoClient,
}

impl SshProblem {
    /// The one line the page shows.
    pub fn line(self) -> &'static str {
        match self {
            SshProblem::NotAnswering => "Not answering",
            SshProblem::NotPaired => "Not paired with this computer",
            SshProblem::IdentityChanged => "Its identity changed. Pair again",
            SshProblem::NotFound => "Can't find it on the network",
            SshProblem::NoClient if cfg!(windows) => "This PC is missing Windows' SSH client",
            SshProblem::NoClient => "This computer has no SSH client",
        }
    }
}

/// Pure: ssh's own words sorted into a case, None when they are none of them. The checks go from the most telling:
/// a host key problem ends the login before any key is offered, and a refused key comes before the connection closes.
/// The words are ssh's own shapes, not loose ones: the stderr also carries what a command on the host printed (a shell's
/// "flare: Permission denied" for a Flare path that is not runnable, a program's "Connection refused"), and that must
/// never read as "Not paired" or "Not answering" (nor end Add's search of the other Flare paths).
pub fn sort(said: &str) -> Option<SshProblem> {
    let t = said.to_ascii_lowercase();
    let any = |ps: &[&str]| ps.iter().any(|p| t.contains(p));
    if any(&[
        "host key verification failed",
        "remote host identification has changed",
        "host key is known for",
        "has changed and you have requested strict checking",
    ]) {
        Some(SshProblem::IdentityChanged)
    } else if any(&[
        // ssh lists the methods it tried: "Permission denied (publickey,...)".
        "permission denied (",
        "too many authentication failures",
        "no supported authentication methods",
    ]) {
        Some(SshProblem::NotPaired)
    } else if any(&["ssh: could not resolve hostname"]) {
        Some(SshProblem::NotFound)
    } else if any(&[
        // "ssh: connect to host <h> port <p>: Connection refused / timed out / No route to host / ...".
        "ssh: connect to host",
        "connection timed out during banner exchange",
        "kex_exchange_identification",
        "connection reset by",
        "connection closed by",
    ]) {
        Some(SshProblem::NotAnswering)
    } else {
        None
    }
}

/// Pure: a failed link's message sorted: ssh missing, or what follows [`SAID`]. Flare's own words before it are
/// never read, so a timeout of Flare's own is not taken for ssh's.
pub fn sort_down(message: &str) -> Option<SshProblem> {
    if message.starts_with(NO_CLIENT) {
        return Some(SshProblem::NoClient);
    }
    message
        .find(SAID)
        .and_then(|at| sort(&message[at + SAID.len()..]))
}

/// Pure: a line of ssh's output into the kept tail: trimmed, cut to [`LINE_MAX`] characters, blank lines dropped,
/// and only the last [`TAIL_LINES`] kept.
pub fn keep_line(tail: &mut VecDeque<String>, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    tail.push_back(line.chars().take(LINE_MAX).collect());
    while tail.len() > TAIL_LINES {
        tail.pop_front();
    }
}

/// ssh's stderr, read on a thread of its own to its end (so ssh never blocks on a full pipe), its last lines kept.
pub struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    ended: mpsc::Receiver<()>,
}

impl StderrTail {
    /// Starts reading `r` on a named thread. A thread that cannot start leaves the tail empty.
    pub fn read(r: impl Read + Send + 'static) -> Self {
        let lines: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let (tx, ended) = mpsc::channel();
        let kept = Arc::clone(&lines);
        let started = std::thread::Builder::new()
            .name("flare-ssh-stderr".into())
            .spawn(move || {
                let mut r = BufReader::new(r);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match r.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => keep_line(
                            &mut kept.lock().unwrap_or_else(PoisonError::into_inner),
                            &String::from_utf8_lossy(&buf),
                        ),
                    }
                }
                let _ = tx.send(());
            });
        if let Err(e) = started {
            log::warn!("could not read ssh's stderr: {e}");
        }
        Self { lines, ended }
    }

    /// BLOCKING (up to `wait`): the kept lines, one per line, once the pipe ended or `wait` is over. A worker's call.
    pub fn text(&self, wait: Duration) -> String {
        let _ = self.ended.recv_timeout(wait); // sync-ok: a link worker waits for ssh's last words, capped by `wait`
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1250: each thing ssh says when a link fails is one plain line; anything else is no case.
    #[test]
    fn what_ssh_says_is_sorted_into_one_plain_line() {
        let cases: [(&str, Option<SshProblem>); 19] = [
            // What a shell or a program on the host printed is no case of ssh's: Add tries the next Flare path.
            ("bash: /usr/local/bin/flare: Permission denied", None),
            ("zsh:1: permission denied: /opt/flare/flare", None),
            (
                "flare: could not reach the daemon: Connection refused",
                None,
            ),
            ("curl: (28) Operation timed out", None),
            (
                "ssh: connect to host studio port 22: Network is unreachable",
                Some(SshProblem::NotAnswering),
            ),
            (
                "ssh: connect to host 192.168.0.139 port 22: Connection refused",
                Some(SshProblem::NotAnswering),
            ),
            (
                "ssh: connect to host studio port 22: Connection timed out",
                Some(SshProblem::NotAnswering),
            ),
            (
                "ssh: connect to host 10.0.0.9 port 22: No route to host",
                Some(SshProblem::NotAnswering),
            ),
            (
                "kex_exchange_identification: read: Connection reset by peer",
                Some(SshProblem::NotAnswering),
            ),
            (
                "max@192.168.0.139: Permission denied (publickey,keyboard-interactive).",
                Some(SshProblem::NotPaired),
            ),
            (
                "Received disconnect from 10.0.0.2 port 22:2: Too many authentication failures",
                Some(SshProblem::NotPaired),
            ),
            (
                "@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
                 Host key verification failed.",
                Some(SshProblem::IdentityChanged),
            ),
            (
                "No ED25519 host key is known for studio and you have requested strict checking.\n\
                 Host key verification failed.",
                Some(SshProblem::IdentityChanged),
            ),
            (
                "ssh: Could not resolve hostname studio: Name or service not known",
                Some(SshProblem::NotFound),
            ),
            (
                "ssh: Could not resolve hostname studio: nodename nor servname provided, or not known",
                Some(SshProblem::NotFound),
            ),
            (
                "ssh: Could not resolve hostname studio: No such host is known. ",
                Some(SshProblem::NotFound),
            ),
            ("Welcome to the machine", None),
            ("", None),
            // A refused key is said before the connection closes: the key is the reason.
            (
                "max@mini: Permission denied (publickey).\nConnection closed by 10.0.0.5 port 22",
                Some(SshProblem::NotPaired),
            ),
        ];
        for (said, want) in cases {
            assert_eq!(sort(said), want, "{said}");
        }
        let lines: Vec<&str> = [
            SshProblem::NotAnswering,
            SshProblem::NotPaired,
            SshProblem::IdentityChanged,
            SshProblem::NotFound,
        ]
        .iter()
        .map(|p| p.line())
        .collect();
        assert_eq!(
            lines,
            [
                "Not answering",
                "Not paired with this computer",
                "Its identity changed. Pair again",
                "Can't find it on the network"
            ]
        );
        assert!(SshProblem::NoClient.line().contains("SSH client"));
        for p in [
            SshProblem::NotAnswering,
            SshProblem::NoClient,
            SshProblem::NotFound,
        ] {
            assert!(!p.line().contains('\u{2014}'), "no em-dash");
        }
    }

    /// Only ssh's own words are read: Flare's timeout before them is no case; a missing client is its mark.
    #[test]
    fn only_what_follows_ssh_said_is_sorted() {
        assert_eq!(sort_down("remote.hello timed out after 90 s"), None);
        assert_eq!(
            sort_down(&format!(
                "the link ended; {SAID}ssh: connect to host x port 22: Connection refused"
            )),
            Some(SshProblem::NotAnswering)
        );
        assert_eq!(
            sort_down(&format!("{NO_CLIENT}could not start the link: not found")),
            Some(SshProblem::NoClient)
        );
        assert_eq!(sort_down(&format!("x; {SAID}hello")), None);
    }

    #[test]
    fn the_tail_keeps_the_last_lines_trimmed_and_cut() {
        let mut t = VecDeque::new();
        for i in 0..20 {
            keep_line(&mut t, &format!("  line {i}\r\n"));
        }
        keep_line(&mut t, "   ");
        assert_eq!(t.len(), TAIL_LINES);
        assert_eq!(t.front().map(String::as_str), Some("line 8"));
        assert_eq!(t.back().map(String::as_str), Some("line 19"));
        keep_line(&mut t, &"x".repeat(LINE_MAX * 2));
        assert_eq!(t.back().map(|l| l.chars().count()), Some(LINE_MAX));
    }

    /// The tail reads a pipe to its end on its own thread and hands its last lines over, invalid UTF-8 included.
    #[test]
    fn the_tail_reads_a_pipe_to_its_end() {
        let mut bytes = b"one\nssh: connect to host h port 22: Connection refused\n".to_vec();
        bytes.extend_from_slice(&[0xff, b'\n']);
        let tail = StderrTail::read(std::io::Cursor::new(bytes));
        let text = tail.text(Duration::from_secs(5));
        assert_eq!(
            text.lines().take(2).collect::<Vec<_>>(),
            ["one", "ssh: connect to host h port 22: Connection refused"]
        );
        assert_eq!(text.lines().count(), 3);
        assert_eq!(sort(&text), Some(SshProblem::NotAnswering));
    }
}
