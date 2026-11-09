//! The Parakeet one-pass runner (Task 14): one `parakeet-cli.exe` per pass, the WAV on its stdin and the
//! text off its stdout. Ported from `parakeetOnce` in Prism Terminal's `dictationEngine.ts` (e5848cb).
//!
//! MEASURED there: a fresh process loads Parakeet v3 (q8_0) and answers an 11 s clip in 0.73 s, a
//! fraction of a resident Whisper's time but nearly all of it start-up. The resident host (`host.rs`)
//! removes that start-up; this is what answers when the host cannot start.
//!
//! `parakeet-cli.exe -m <model> -f - -np -t <threads>` plus `-ng` on the CPU engine. The language is not
//! sent (Parakeet detects it). The audio never touches the disk: `-f -` is stdin. A pass that has not
//! answered within the request timeout is killed.

use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use super::whisper::{push_tail, Answer};
use super::EngineKind;
use crate::job::Job;

/// `CREATE_NO_WINDOW`: a console engine must never flash a window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How often a running pass is asked whether it is done.
const POLL: Duration = Duration::from_millis(5);
/// How long the readers are waited for once the process is gone (a pipe something else holds open).
const DRAIN: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The argv of one pass.
pub fn pass_args(model: &Path, threads: u32, kind: EngineKind) -> Vec<String> {
    let mut args: Vec<String> = [
        "-m",
        &crate::ansi_path::engine_safe_path(model).to_string_lossy(),
        "-f",
        "-",
        "-np",
        "-t",
        &threads.to_string(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    if kind == EngineKind::Cpu {
        args.push("-ng".to_string());
    }
    args
}

/// The pass in flight, so `stop()` can end it.
pub struct Pass {
    child: Mutex<Child>,
    pub pid: u32,
}

impl Pass {
    /// Ends the process (TerminateProcess).
    pub fn kill(&self) {
        let _ = lock(&self.child).kill();
    }
}

/// What to run for one pass.
pub struct Spec<'a> {
    pub file: &'a Path,
    pub args: &'a [String],
    pub dir: &'a Path,
    pub env: &'a [(String, String)],
    pub wav: &'a [u8],
    pub timeout: Duration,
}

/// Runs one pass to its end. `register` is told about the child once it is in the job, so the engine can
/// kill it on `stop()`. Never panics, never leaves the child running.
pub fn run(spec: &Spec<'_>, job: &Job, register: &dyn Fn(&Arc<Pass>)) -> Answer {
    let mut cmd = Command::new(spec.file);
    cmd.args(spec.args)
        .current_dir(spec.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    // macOS: a process group of its own, so the reaper ends the child and all it starts (rule 312).
    crate::job::isolate(&mut cmd);
    for (k, v) in spec.env {
        cmd.env(k, v);
    }
    // sync-ok: runs on the engine's worker thread, never the UI thread
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Answer::Failed {
                detail: e.to_string(),
                died: true,
            }
        }
    };
    if let Err(e) = job.assign(&child) {
        // Outside the kill-on-close job a pass could outlive the app: never run one.
        let _ = child.kill();
        let _ = child.wait();
        return Answer::Failed {
            detail: format!("the dictation engine could not join the kill-on-close job: {e}"),
            died: true,
        };
    }
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let pid = child.id();
    let member = job.member();
    let pass = Arc::new(Pass {
        child: Mutex::new(child),
        pid,
    });
    register(&pass);

    // The audio goes in on its own thread: a child that never reads would block this one for good. A
    // child that died before reading all of it closes the pipe under the write; its exit is the answer.
    let wav = spec.wav.to_vec();
    thread::spawn(move || {
        if let Some(mut pipe) = stdin {
            let _ = pipe.write_all(&wav);
        }
    });
    let out_rx = read_all(stdout);
    let err_rx = read_tail(stderr);

    let deadline = Instant::now() + spec.timeout;
    let status = loop {
        let polled = member.try_wait(&mut lock(&pass.child));
        match polled {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            pass.kill();
            let _ = member.wait(&mut lock(&pass.child));
            return Answer::Failed {
                detail: format!("no answer after {} ms", spec.timeout.as_millis()),
                died: false,
            };
        }
        thread::sleep(POLL);
    };
    // The readers see end-of-file once the process is gone; by then stdout and stderr are read out.
    let stdout_bytes = out_rx.recv_timeout(DRAIN).unwrap_or_default();
    let stderr_text = err_rx.recv_timeout(DRAIN).unwrap_or_default();
    judge(status, &stdout_bytes, &stderr_text)
}

/// What a finished pass came to: the exit code, what it printed and what it said.
pub fn judge(code: Option<i32>, stdout: &[u8], stderr: &str) -> Answer {
    let said = stderr.trim();
    if code != Some(0) {
        let detail = if said.is_empty() {
            match code {
                Some(c) => format!("parakeet-cli exited with code {c}"),
                None => "parakeet-cli exited abnormally".to_string(),
            }
        } else {
            said.to_string()
        };
        return Answer::Failed { detail, died: true };
    }
    // MEASURED: audio it cannot read is reported on stderr as "error: ..." with exit code 0, so the code
    // alone is not the verdict.
    if let Some(line) = stderr
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .find(|l| l.starts_with("error:"))
    {
        return Answer::Failed {
            detail: line.to_string(),
            died: false,
        };
    }
    // Its own UTF-8 bytes, untouched by any code page: the languages it knows are full of letters
    // outside ASCII. CRLF and the edges trimmed.
    Answer::Text(String::from_utf8_lossy(stdout).trim().to_string())
}

fn read_all(pipe: Option<std::process::ChildStdout>) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

fn read_tail(pipe: Option<std::process::ChildStderr>) -> Receiver<String> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut tail = String::new();
        if let Some(mut p) = pipe {
            let mut buf = [0u8; 4096];
            loop {
                match p.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => push_tail(&mut tail, &String::from_utf8_lossy(&buf[..n])),
                }
            }
        }
        let _ = tx.send(tail);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(a: Answer) -> (String, bool) {
        match a {
            Answer::Failed { detail, died } => (detail, died),
            Answer::Text(t) => panic!("expected a failure, got text {t:?}"),
        }
    }

    #[test]
    fn the_argv_is_the_models_stdin_no_prints_and_threads_with_ng_on_the_cpu_only() {
        let m = Path::new(r"C:\m\p.bin");
        assert_eq!(
            pass_args(m, 6, EngineKind::Cpu),
            ["-m", r"C:\m\p.bin", "-f", "-", "-np", "-t", "6", "-ng"]
        );
        assert_eq!(
            pass_args(m, 3, EngineKind::Gpu),
            ["-m", r"C:\m\p.bin", "-f", "-", "-np", "-t", "3"]
        );
    }

    #[test]
    fn text_is_utf8_with_the_edges_trimmed() {
        let bytes = "  Gr\u{fc}\u{df} Gott \r\n".as_bytes();
        assert!(matches!(judge(Some(0), bytes, ""), Answer::Text(t) if t == "Gr\u{fc}\u{df} Gott"));
    }

    #[test]
    fn a_bad_exit_says_what_stderr_said_else_the_code() {
        let (d, died) = failed(judge(Some(3), b"", "  CUDA error\n"));
        assert_eq!((d.as_str(), died), ("CUDA error", true));
        let (d, died) = failed(judge(Some(9), b"", ""));
        assert_eq!(
            (d.as_str(), died),
            ("parakeet-cli exited with code 9", true)
        );
        let (d, _) = failed(judge(None, b"", ""));
        assert!(d.contains("exited"));
    }

    #[test]
    fn error_with_exit_0_is_a_failure_that_did_not_die() {
        let (d, died) = failed(judge(
            Some(0),
            b"",
            "load\r\nerror: failed to read audio file '-'\r\nmore",
        ));
        assert_eq!(d, "error: failed to read audio file '-'");
        assert!(!died);
        // "error" inside a line is not the verdict.
        assert!(matches!(
            judge(Some(0), b"hi", "no error: here"),
            Answer::Text(_)
        ));
    }
}
