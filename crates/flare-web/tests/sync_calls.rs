//! Scan: a blocking call in `src` (outside test code) must say why it is allowed (rule 34). The web host runs on the UI thread; its slow question, the runtime check, runs on a worker. The two calls that cannot leave the UI thread (WebView2 objects are thread-affine to its STA) are listed below and marked, so a new one cannot slip in unseen.

use std::path::{Path, PathBuf};

const BLOCKING: &[&str] = &[
    "File::open",
    "read_to_string",
    "sleep(",
    "Command::new",
    "std::fs::",
    "fs::read",
    "fs::write",
    "fs::metadata",
    ".output()",
    ".status()",
    ".wait(",
    ".recv()",
    ".recv_timeout(",
    "thread::sleep",
    ".join()",
    ".wait_timeout(",
    ".wait_while(",
    // The browser's own calls: registry reads, a library load, the data folder and the browser launch
    // happen inside them, on the UI thread's apartment.
    "CreateCoreWebView2EnvironmentWithOptions(",
    "CreateCoreWebView2Controller(",
];

const MARK: &str = "// sync-ok:";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // `bin` holds the binaries' mains; `probe` is the macOS web host tests' own browser process (it runs
            // in a dev bundle for the tests, never in the window, and waits on purpose).
            if p.file_name().is_some_and(|n| n == "bin" || n == "probe") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs")
            && p.file_name().is_some_and(|n| n != "fake.rs")
        {
            out.push(p);
        }
    }
}

fn src_files() -> (PathBuf, Vec<PathBuf>) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    (src, files)
}

/// True when the marker sits on the call's line, on the comment line right above it, or within
/// the next three lines (rustfmt splits a method chain, so the trailing marker can land there).
fn marked(lines: &[&str], i: usize) -> bool {
    if lines[i].contains(MARK) {
        return true;
    }
    if i > 0 && lines[i - 1].trim_start().starts_with("//") && lines[i - 1].contains(MARK) {
        return true;
    }
    lines.iter().skip(i + 1).take(3).any(|l| l.contains(MARK))
}

/// Every blocking-call line before the file's first `#[cfg(test)]` that carries no marker:
/// (file, line no, text).
fn unmarked_blocking_lines() -> Vec<(String, usize, String)> {
    let (src, files) = src_files();
    let mut hits = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        let name = f
            .strip_prefix(&src)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("#[cfg(test)]") {
                break;
            }
            if line.trim_start().starts_with("//") {
                continue;
            }
            if BLOCKING.iter().any(|p| line.contains(p)) && !marked(&lines, i) {
                hits.push((name.clone(), i + 1, line.trim().to_string()));
            }
        }
    }
    hits
}

#[test]
fn no_unmarked_blocking_calls_in_src() {
    let offenders: Vec<String> = unmarked_blocking_lines()
        .into_iter()
        .map(|(f, n, t)| format!("{f}:{n}: {t}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "blocking calls without a `// sync-ok: <reason>` comment:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_sync_ok_has_a_reason() {
    let (_, files) = src_files();
    let mut bad = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//!") || t.starts_with("///") {
                continue; // docs may mention the marker
            }
            if let Some(at) = line.find("sync-ok:") {
                let reason = line[at + "sync-ok:".len()..].trim();
                if reason.chars().count() < 8 {
                    bad.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "sync-ok without a reason:\n{}",
        bad.join("\n")
    );
}
