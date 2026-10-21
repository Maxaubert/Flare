//! The resident whisper-server child (Task 13): spawn it, watch it, kill it, read why it died. Ported
//! from the Server parts of Prism Terminal's `dictationEngine.ts` (e5848cb).
//!
//! A server lives from spawn to the moment its process is gone AND its stderr has been read out: the
//! reason a server died is on stderr, and the exit can be reported before the pipe has drained, so
//! "exited" is only announced once stderr has closed (or 250 ms after the exit, for a pipe something
//! else still holds open).

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::EngineKind;
use crate::job::Job;

/// How much of the server's stderr is kept: its last few lines, which is where it says why it died.
pub const STDERR_KEEP: usize = 2000;
/// `CREATE_NO_WINDOW`: a console engine must never flash a window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long after the process is gone a pipe that stays open is waited for.
const STDERR_GRACE: Duration = Duration::from_millis(250);
/// How often the monitor asks the child whether it is gone.
const MONITOR_POLL: Duration = Duration::from_millis(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct State {
    stderr: String,
    stderr_closed: bool,
    exit_code: Option<i32>,
    exited: bool,
}

/// One launched server.
pub struct Server {
    pub kind: EngineKind,
    pub model_path: PathBuf,
    /// The catalogue id the server was launched for: the caller-side warm check compares ids, never paths.
    pub model_id: String,
    pub language: String,
    pub port: u16,
    pub pid: u32,
    child: Mutex<Child>,
    state: Mutex<State>,
    changed: Condvar,
    warm: AtomicBool,
}

/// Appends `chunk` to `tail` and keeps only the last [`STDERR_KEEP`] characters.
pub fn push_tail(tail: &mut String, chunk: &str) {
    tail.push_str(chunk);
    let chars = tail.chars().count();
    if chars > STDERR_KEEP {
        let drop_chars = chars - STDERR_KEEP;
        if let Some((at, _)) = tail.char_indices().nth(drop_chars) {
            tail.drain(..at);
        }
    }
}

impl Server {
    /// Starts `file args` in `dir` with `env`, joins it to the job, and starts watching it.
    pub fn spawn(
        file: &Path,
        args: &[String],
        dir: &Path,
        env: &[(String, String)],
        job: &Job,
        meta: (EngineKind, PathBuf, String, String, u16),
    ) -> std::io::Result<Arc<Server>> {
        let mut cmd = Command::new(file);
        cmd.args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        if let Err(e) = job.assign(&child) {
            // Outside the kill-on-close job a server could outlive the app: never run one.
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                e.kind(),
                format!("the dictation engine could not join the kill-on-close job: {e}"),
            ));
        }
        let stderr = child.stderr.take();
        let pid = child.id();
        let server = Arc::new(Server {
            kind: meta.0,
            model_path: meta.1,
            model_id: meta.2,
            language: meta.3,
            port: meta.4,
            pid,
            child: Mutex::new(child),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            warm: AtomicBool::new(false),
        });
        let reader = Arc::clone(&server);
        thread::spawn(move || {
            if let Some(mut pipe) = stderr {
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                            push_tail(&mut lock(&reader.state).stderr, &text);
                        }
                    }
                }
            }
            lock(&reader.state).stderr_closed = true;
            reader.changed.notify_all();
        });
        let monitor = Arc::clone(&server);
        thread::spawn(move || {
            let code = loop {
                let polled = lock(&monitor.child).try_wait();
                match polled {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => thread::sleep(MONITOR_POLL),
                    Err(_) => break None,
                }
            };
            let mut st = lock(&monitor.state);
            st.exit_code = code;
            let (mut st, _) = monitor
                .changed
                .wait_timeout_while(st, STDERR_GRACE, |s| !s.stderr_closed)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            st.exited = true;
            drop(st);
            monitor.changed.notify_all();
        });
        Ok(server)
    }

    /// Ends the process (TerminateProcess: no signal to ignore, and whisper-server spawns nothing).
    pub fn kill(&self) {
        let _ = lock(&self.child).kill();
    }

    /// The process is gone and its stderr has been read out.
    pub fn exited(&self) -> bool {
        lock(&self.state).exited
    }

    /// Waits up to `d` for the exit; true when it has exited.
    pub fn wait_exit(&self, d: Duration) -> bool {
        let st = lock(&self.state);
        let (st, _) = self
            .changed
            .wait_timeout_while(st, d, |s| !s.exited)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.exited
    }

    /// A pass has come back from it: the model is on the device and the kernels are compiled.
    pub fn warm(&self) -> bool {
        self.warm.load(Ordering::SeqCst)
    }

    /// Marks it warm.
    pub fn set_warm(&self) {
        self.warm.store(true, Ordering::SeqCst);
    }

    /// What it said last (its last [`STDERR_KEEP`] characters), trimmed.
    pub fn stderr(&self) -> String {
        lock(&self.state).stderr.trim().to_string()
    }

    /// Why it died: its stderr, else its exit code.
    pub fn why_it_died(&self) -> String {
        let said = self.stderr();
        if !said.is_empty() {
            return said;
        }
        match lock(&self.state).exit_code {
            Some(code) => format!("the server exited with code {code}"),
            None => "the server exited".to_string(),
        }
    }
}

/// The upload, built by hand: two parts and a boundary no WAV will contain by accident (24 random hex
/// digits). Returns the body and the `Content-Type` value.
pub fn multipart(wav: &[u8]) -> (Vec<u8>, String) {
    let boundary = format!("----FlareDictation{}", random_hex());
    let mut body = Vec::with_capacity(wav.len() + 400);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n--{boundary}--\r\n"
        )
        .as_bytes(),
    );
    (body, format!("multipart/form-data; boundary={boundary}"))
}

/// 24 hex digits that differ per call (a boundary needs to be unlikely in a clip, not secret).
fn random_hex() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut h = Sha256::new();
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    h.update(format!("{:?}", Instant::now()).as_bytes());
    h.finalize()
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What a pass came to.
pub enum Answer {
    Text(String),
    Failed { detail: String, died: bool },
}

/// Reads the server's answer to a pass: HTTP 200 with JSON `{"text": "..."}`.
pub fn read_answer(status: u16, body: &[u8]) -> Answer {
    let raw = String::from_utf8_lossy(body);
    let cut = |s: &str| s.chars().take(300).collect::<String>();
    if status != 200 {
        return Answer::Failed {
            detail: format!("HTTP {status}: {}", cut(&raw)),
            died: false,
        };
    }
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => match v.get("text").and_then(serde_json::Value::as_str) {
            Some(text) => Answer::Text(text.to_string()),
            None => Answer::Failed {
                detail: format!("no text in the answer: {}", cut(&raw)),
                died: false,
            },
        },
        Err(_) => Answer::Failed {
            detail: format!("not JSON: {}", cut(&raw)),
            died: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stderr_tail_keeps_the_last_2000_characters() {
        let mut tail = String::new();
        push_tail(&mut tail, &"a".repeat(1500));
        push_tail(&mut tail, &"b".repeat(1500));
        assert_eq!(tail.chars().count(), STDERR_KEEP);
        assert!(tail.ends_with('b') && tail.starts_with('a'));
        let mut wide = String::new();
        push_tail(&mut wide, &"\u{e6}".repeat(2500));
        assert_eq!(wide.chars().count(), STDERR_KEEP);
    }

    #[test]
    fn the_upload_is_two_parts_with_a_fresh_boundary() {
        let wav = [7u8; 10];
        let (a, ct_a) = multipart(&wav);
        let (_, ct_b) = multipart(&wav);
        assert_ne!(ct_a, ct_b);
        let boundary = ct_a.split("boundary=").nth(1).unwrap_or_default();
        let text = String::from_utf8_lossy(&a);
        assert!(text.contains("name=\"file\"; filename=\"audio.wav\""));
        assert!(text.contains("Content-Type: audio/wav"));
        assert!(text.contains("name=\"response_format\"\r\n\r\njson"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        assert_eq!(boundary.len(), "----FlareDictation".len() + 24);
    }

    #[test]
    fn an_answer_is_text_or_a_reason() {
        assert!(matches!(
            read_answer(200, br#"{"text":" hi "}"#),
            Answer::Text(t) if t == " hi "
        ));
        for (status, body, needle) in [
            (500, &b"boom"[..], "HTTP 500: boom"),
            (200, &b"nope"[..], "not JSON"),
            (200, &br#"{"x":1}"#[..], "no text in the answer"),
        ] {
            match read_answer(status, body) {
                Answer::Failed { detail, died } => {
                    assert!(detail.contains(needle), "{detail}");
                    assert!(!died);
                }
                Answer::Text(_) => panic!("expected a failure"),
            }
        }
    }
}
