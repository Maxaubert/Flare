//! The resident Parakeet host, client side (Task 14, N6 and N7, D3). Ported from `startHost`, `hostOnce`
//! and `killHost` in Prism Terminal's `dictationEngine.ts` (e5848cb); the host itself is
//! `flare-parakeet-host.exe` (`src/bin/flare_parakeet_host.rs`).
//!
//! The protocol, byte for byte as Prism Terminal's `parakeetHost.ps1` speaks it:
//! * argv `<engine dir> <model> <gpu 1|0> <threads> [<cache>]`;
//! * stdin: `pass <n>` LF, then n bytes of a 16 kHz mono PCM16 WAV; `quit` ends it;
//! * stdout: `ready` once the model is loaded, then per pass `ok <base64 of the UTF-8 text>` or
//!   `err <why>`. A line that is none of those three words (a native library printing) is ignored.
//!
//! Like a Whisper server, a host lives from spawn to the moment its process is gone AND its stderr has
//! been read out: the reason it died is on stderr, and "exited" is only announced once the pipes have
//! closed (or 250 ms after the exit, for a pipe something else still holds open).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::whisper::{push_tail, Answer};
use super::EngineKind;
use crate::job::Job;

/// `CREATE_NO_WINDOW`: a console engine must never flash a window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long after the process is gone a pipe that stays open is waited for.
const PIPE_GRACE: Duration = Duration::from_millis(250);
/// How often the monitor asks the child whether it is gone.
const MONITOR_POLL: Duration = Duration::from_millis(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Only its own words: `ready`, `ok` and `err`, each as a whole word (`^(ready|ok|err)\b`).
pub fn is_protocol_line(line: &str) -> bool {
    ["ready", "ok", "err"].iter().any(|word| {
        line.strip_prefix(word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

/// What one protocol line means as the answer to a pass: `ok <base64>` is the text (`ok ` alone is the
/// empty text), anything else is the host's reason (`err <why>` gives `<why>`).
pub fn answer_from_line(line: &str) -> Answer {
    if line.starts_with("ok") {
        let encoded = line.get(3..).unwrap_or("").trim();
        return match STANDARD.decode(encoded) {
            Ok(bytes) => Answer::Text(String::from_utf8_lossy(&bytes).trim().to_string()),
            Err(_) => Answer::Failed {
                detail: "the Parakeet host sent an unreadable answer".to_string(),
                died: false,
            },
        };
    }
    Answer::Failed {
        detail: line.get(4..).unwrap_or("").to_string(),
        died: false,
    }
}

/// What waiting for the host's next line came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Line(String),
    /// The host exited first.
    Gone,
    Timeout,
}

#[derive(Default)]
struct State {
    lines: VecDeque<String>,
    stderr: String,
    out_closed: bool,
    err_closed: bool,
    exited: bool,
}

/// One launched host: one model on one engine, kept loaded.
pub struct Host {
    pub kind: EngineKind,
    pub dir: PathBuf,
    pub model_path: PathBuf,
    pub pid: u32,
    child: Mutex<Child>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    state: Mutex<State>,
    changed: Condvar,
    killed: AtomicBool,
}

impl Host {
    /// Starts `file args` in `dir` with `env`, joins it to the job, and starts watching it.
    pub fn spawn(
        file: &Path,
        args: &[String],
        dir: &Path,
        env: &[(String, String)],
        job: &Job,
        meta: (EngineKind, PathBuf, PathBuf),
    ) -> std::io::Result<Arc<Host>> {
        let mut cmd = Command::new(file);
        cmd.args(args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        // macOS: a process group of its own, so the reaper ends the child and all it starts (rule 1081).
        crate::job::isolate(&mut cmd);
        for (k, v) in env {
            cmd.env(k, v);
        }
        // sync-ok: runs on the engine's worker thread, never the UI thread
        let mut child = cmd.spawn()?;
        if let Err(e) = job.assign(&child) {
            // Outside the kill-on-close job a host could outlive the app: never run one.
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                e.kind(),
                format!("the dictation engine could not join the kill-on-close job: {e}"),
            ));
        }
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let pid = child.id();
        let member = job.member();
        let host = Arc::new(Host {
            kind: meta.0,
            dir: meta.1,
            model_path: meta.2,
            pid,
            child: Mutex::new(child),
            stdin: Arc::new(Mutex::new(stdin)),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            killed: AtomicBool::new(false),
        });

        let reader = Arc::clone(&host);
        thread::spawn(move || {
            if let Some(pipe) = stdout {
                let mut pipe = BufReader::new(pipe);
                let mut raw = Vec::new();
                loop {
                    raw.clear();
                    match pipe.read_until(b'\n', &mut raw) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&raw).trim().to_string();
                            if is_protocol_line(&line) {
                                lock(&reader.state).lines.push_back(line);
                                reader.changed.notify_all();
                            }
                        }
                    }
                }
            }
            lock(&reader.state).out_closed = true;
            reader.changed.notify_all();
        });
        let errs = Arc::clone(&host);
        thread::spawn(move || {
            if let Some(mut pipe) = stderr {
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                            push_tail(&mut lock(&errs.state).stderr, &text);
                        }
                    }
                }
            }
            lock(&errs.state).err_closed = true;
            errs.changed.notify_all();
        });
        let monitor = Arc::clone(&host);
        thread::spawn(move || {
            loop {
                let polled = member.try_wait(&mut lock(&monitor.child));
                match polled {
                    Ok(None) => thread::sleep(MONITOR_POLL),
                    Ok(Some(_)) | Err(_) => break,
                }
            }
            let st = lock(&monitor.state);
            let (mut st, _) = monitor
                .changed
                .wait_timeout_while(st, PIPE_GRACE, |s| !(s.out_closed && s.err_closed))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            st.exited = true;
            drop(st);
            monitor.changed.notify_all();
        });
        Ok(host)
    }

    /// Ends the process (TerminateProcess). Idempotent; never waits on a write in progress.
    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        // Letting go of stdin is the polite half (EOF ends a host that is idle); a write in progress
        // keeps its lock and its pipe, and the kill below breaks it.
        if let Ok(mut pipe) = self.stdin.try_lock() {
            pipe.take();
        }
        let _ = lock(&self.child).kill();
    }

    /// `kill` was called on it (a timeout, a stop, a replacement).
    pub fn was_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// The process is gone and its stderr has been read out.
    pub fn exited(&self) -> bool {
        lock(&self.state).exited
    }

    /// What it said on stderr (its last 2000 characters), trimmed.
    pub fn stderr(&self) -> String {
        lock(&self.state).stderr.trim().to_string()
    }

    /// Its next protocol line, `Gone` when it exits first, `Timeout` at `deadline`. Lines it said before
    /// it exited are still delivered.
    pub fn next_line(&self, deadline: Instant) -> Next {
        let mut st = lock(&self.state);
        loop {
            if let Some(line) = st.lines.pop_front() {
                return Next::Line(line);
            }
            if st.exited {
                return Next::Gone;
            }
            let now = Instant::now();
            if now >= deadline {
                return Next::Timeout;
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// Waits for `ready`. A host that says anything else first (`err the model did not load`), exits, or
    /// is not ready within `timeout` is not ready; one that timed out is killed.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        match self.next_line(Instant::now() + timeout) {
            Next::Line(line) => line == "ready",
            Next::Gone => false,
            Next::Timeout => {
                self.kill();
                false
            }
        }
    }

    /// One pass: `pass <n>` and the bytes, then the answer. A host that does not answer within `timeout`
    /// is killed (the next pass starts a fresh one). Never blocks on the write: a host that stops reading
    /// would otherwise hold the timeout hostage.
    pub fn pass(&self, wav: &[u8], timeout: Duration) -> Answer {
        let deadline = Instant::now() + timeout;
        let pipe = Arc::clone(&self.stdin);
        let bytes = wav.to_vec();
        thread::spawn(move || {
            let mut guard = lock(&pipe);
            if let Some(w) = guard.as_mut() {
                let _ = w
                    .write_all(format!("pass {}\n", bytes.len()).as_bytes())
                    .and_then(|()| w.write_all(&bytes))
                    .and_then(|()| w.flush());
            }
        });
        match self.next_line(deadline) {
            Next::Line(line) => answer_from_line(&line),
            Next::Gone => {
                let said = self.stderr();
                Answer::Failed {
                    detail: if said.is_empty() {
                        "the Parakeet host exited".to_string()
                    } else {
                        said
                    },
                    died: true,
                }
            }
            Next::Timeout => {
                self.kill();
                Answer::Failed {
                    detail: format!("no answer after {} ms", timeout.as_millis()),
                    died: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_its_own_words_are_protocol() {
        for yes in [
            "ready",
            "ok ",
            "ok",
            "ok QQ==",
            "err boom",
            "err",
            "ready\u{b7}",
        ] {
            assert!(is_protocol_line(yes), "{yes:?}");
        }
        for no in [
            "",
            "okay then",
            "readyish",
            "error: x",
            "loading model...",
            " ok",
            "ok_",
            "[ggml] ok",
        ] {
            assert!(!is_protocol_line(no), "{no:?}");
        }
    }

    #[test]
    fn an_ok_line_is_the_text_and_an_empty_ok_is_the_empty_text() {
        let encoded = STANDARD.encode("  Gr\u{fc}\u{df} Gott \n".as_bytes());
        assert!(matches!(
            answer_from_line(&format!("ok {encoded}")),
            Answer::Text(t) if t == "Gr\u{fc}\u{df} Gott"
        ));
        assert!(matches!(answer_from_line("ok"), Answer::Text(t) if t.is_empty()));
        assert!(matches!(answer_from_line("ok "), Answer::Text(t) if t.is_empty()));
    }

    #[test]
    fn an_err_line_is_the_reason_without_its_word() {
        match answer_from_line("err the pass failed (-3)") {
            Answer::Failed { detail, died } => {
                assert_eq!(detail, "the pass failed (-3)");
                assert!(!died);
            }
            Answer::Text(_) => panic!("expected a failure"),
        }
        assert!(matches!(
            answer_from_line("err"),
            Answer::Failed { detail, .. } if detail.is_empty()
        ));
    }

    #[test]
    fn a_garbled_answer_is_a_failure_not_a_panic() {
        assert!(matches!(
            answer_from_line("ok !!!notbase64"),
            Answer::Failed { died: false, .. }
        ));
    }
}
