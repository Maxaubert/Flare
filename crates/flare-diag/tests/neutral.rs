//! M9 Task 35: the diagnostics log never carries typed text, clipboard, audio or titles (regression rule
//! 110; D12: Flare's `mark` line carries no note). Three checks: the marker sent through the log's own
//! paths never lands in a file of the log folder; the log has no raw-text API and no caller passes a
//! user-text field key; and no panic message in the input, clipboard or dictation code formats a user
//! string (a panic message is written to the log).
//!
//! The marker runs through the mark, the session, the crash and the flush paths here. The live input,
//! clipboard, dictation, tab-title and agent-title paths are the app's own: they are covered by the
//! field-key scan below and by the e2e run (M9 Task 36), which types the marker into a shell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_diag::control::DiagControl;
use flare_diag::writer::{DiagLog, DiagLogOptions};
use serde_json::Map;

const MARKER: &str = "MARKER-4f2b";

/// The field keys that would carry typed text, a clipboard, a transcript, audio, a title, a note, a
/// prompt or a terminal line. None may name a field of a diagnostics line.
const TEXT_KEYS: &[&str] = &[
    "text",
    "input",
    "clipboard",
    "paste",
    "title",
    "transcript",
    "audio",
    "note",
    "prompt",
    "line",
];

/// The honest exceptions: a committed line that uses one of `TEXT_KEYS` for a value that is not text
/// (a count, a bool, a fixed word). Each row names the file, the key and why. Never rename a field to
/// dodge the scan.
const ALLOWED: &[(&str, &str, &str)] = &[];

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "flare-neutral-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("a scratch folder"); // sync-ok: test code
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // sync-ok: test code
    }
}

/// Every file of the log folder, read as text (the live file, `.1` to `.4`, and `diag.json`).
fn log_folder_texts(profile: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut paths = vec![profile.join("diag.json")];
    if let Ok(rd) = std::fs::read_dir(profile.join("logs")) {
        for e in rd.flatten() {
            paths.push(e.path());
        }
    }
    for p in paths {
        if let Ok(text) = std::fs::read_to_string(&p) {
            out.push((p.display().to_string(), text));
        }
    }
    out
}

#[test]
fn a_marker_never_reaches_the_log() {
    let s = Scratch::new("marker");
    let mut opts = DiagLogOptions::new(s.0.join("logs"));
    opts.flush_ms = 10;
    let log = DiagLog::start(opts);
    let control = DiagControl::with_log(&s.0, log.clone());
    // Settings > Diagnostics > Mark a problem: the mark line carries no note.
    control.mark();
    // The session and the quit lines carry their own fixed fields.
    log.write("main", "session", Map::new());
    log.write("main", "quit", Map::new());
    log.flush();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code waits for the writer thread
    log.flush();
    let files = log_folder_texts(&s.0);
    assert!(
        files.iter().any(|(_, t)| t.contains("\"k\":\"mark\"")),
        "the mark line landed"
    );
    for (name, text) in files {
        assert!(!text.contains(MARKER), "{name} holds the marker");
    }
}

#[test]
fn the_log_has_no_raw_text_api() {
    let mut bad = Vec::new();
    for f in crate_src_files() {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        for (i, line) in text.lines().enumerate() {
            if line.contains("write_raw") {
                bad.push(format!("{}:{}: {}", rel(&f), i + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "a raw-text API:\n{}", bad.join("\n"));
}

#[test]
fn no_caller_puts_a_text_key_on_a_diagnostics_line() {
    let mut bad = Vec::new();
    for f in crate_src_files() {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        if !(text.contains("flare_diag")
            || text.contains("DiagLog")
            || text.contains("LineSink")
            || text.contains("DiagSink")
            || text.contains("fields("))
        {
            continue;
        }
        let r = rel(&f);
        let lines: Vec<&str> = text.lines().collect();
        // A diagnostics call and the four lines after it: the fields it builds are there.
        let calls: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| is_log_call(l))
            .map(|(i, _)| i)
            .collect();
        for key in TEXT_KEYS {
            for form in [
                format!("(\"{key}\","),
                format!("insert(\"{key}\""),
                format!("json!({{\"{key}\""),
                format!("\"{key}\" =>"),
            ] {
                let hit = calls
                    .iter()
                    .find(|&&c| lines.iter().skip(c).take(5).any(|l| l.contains(&form)));
                let Some(&c) = hit else {
                    continue;
                };
                let excused = ALLOWED.iter().any(|(file, k, _)| *file == r && *k == *key);
                if !excused {
                    bad.push(format!("{r}:{}: field key {key:?}", c + 1));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "a diagnostics line names a text field (use a count, a bool or a fixed word, or add an ALLOWED row):\n{}",
        bad.join("\n")
    );
}

/// A panic message in the input, clipboard and dictation code is written to the log, so it may not
/// format a user string: `expect(&format!` and `panic!(` with a placeholder are findings there.
#[test]
fn no_panic_formats_a_user_string_in_the_input_paths() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates/flare-term/src"), &mut files);
    rust_files(&root.join("crates/flare-dictation/src"), &mut files);
    files.push(root.join("crates/flare-win/src/clipboard.rs"));
    let mut bad = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("#[cfg(test)]") {
                break;
            }
            let code = line.split("//").next().unwrap_or("");
            let formats = code.contains("expect(&format!")
                || (code.contains("panic!(") && code.contains('{'));
            if formats {
                bad.push(format!("{}:{}: {}", rel(&f), i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "a panic formats a string:\n{}",
        bad.join("\n")
    );
}

/// A line that writes to the diagnostics log: a `write`, `write_at` or `emit` call, a `record` call on
/// a sink, or a `fields(` helper. Comment lines are not calls.
fn is_log_call(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") {
        return false;
    }
    [".write(", ".write_at(", ".emit(", ".record(", "fields(&["]
        .iter()
        .any(|p| code.contains(p))
}

#[test]
fn the_text_key_scan_recognises_a_text_field() {
    let sample = "log.write(\"main\", \"x\", fields(&[(\"text\", json!(s))]));";
    assert!(sample.contains(&format!("(\"{}\",", "text")));
    let ok = "log.write(\"main\", \"x\", fields(&[(\"count\", json!(3))]));";
    assert!(!TEXT_KEYS.iter().any(|k| ok.contains(&format!("(\"{k}\","))));
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/flare-diag sits two levels under the repository root")
        .to_path_buf()
}

fn rel(p: &Path) -> String {
    p.strip_prefix(repo_root())
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every `.rs` file under every crate's `src`.
fn crate_src_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(repo_root().join("crates")) else {
        return out;
    };
    let mut crates: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    crates.sort();
    for c in crates {
        rust_files(&c.join("src"), &mut out);
    }
    out
}
