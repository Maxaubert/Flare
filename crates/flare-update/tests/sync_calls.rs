//! M9 Task 35: a blocking call in the update, diagnostics and shell-verb code says why it may block
//! (rule 34; the M2 scan, widened to M9's modules). The modules are the pure update code, the whole
//! diagnostics crate, and the four flare-win modules that touch the registry, the verb, the verb switch
//! and the packaged check.

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
];

const MARK: &str = "// sync-ok:";

/// True when the marker sits on the call's line, on the comment line right above it, or within the
/// next three lines (rustfmt splits a method chain, so a trailing marker can land there).
fn marked(lines: &[&str], i: usize) -> bool {
    if lines[i].contains(MARK) {
        return true;
    }
    if i > 0 && lines[i - 1].trim_start().starts_with("//") && lines[i - 1].contains(MARK) {
        return true;
    }
    lines.iter().skip(i + 1).take(3).any(|l| l.contains(MARK))
}

/// A signature line (`fn sleep(&self, ms: u64)`) is a declaration, not a call, so it is not scanned.
fn is_declaration(line: &str) -> bool {
    let t = line.trim_start();
    let t = t
        .strip_prefix("pub(crate) ")
        .or_else(|| t.strip_prefix("pub "))
        .unwrap_or(t);
    t.starts_with("fn ")
}

/// The unmarked blocking-call lines of one file, before its first `#[cfg(test)]`: (zero-based index, text).
fn unmarked(lines: &[&str]) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("#[cfg(test)]") {
            break;
        }
        if line.trim_start().starts_with("//") || is_declaration(line) {
            continue;
        }
        if BLOCKING.iter().any(|p| line.contains(p)) && !marked(lines, i) {
            hits.push((i, line.trim().to_string()));
        }
    }
    hits
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/flare-update sits two levels under the repository root")
        .to_path_buf()
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

/// The files Task 35 covers.
fn covered() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates/flare-update/src"), &mut files);
    rust_files(&root.join("crates/flare-diag/src"), &mut files);
    for f in ["registry", "verb", "verb_switch", "packaged"] {
        files.push(root.join(format!("crates/flare-win/src/{f}.rs")));
    }
    files.sort();
    files
}

fn rel(p: &Path) -> String {
    p.strip_prefix(repo_root())
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn no_blocking_call_in_flare_update_without_a_reason() {
    let mut bad = Vec::new();
    for f in covered() {
        let text = std::fs::read_to_string(&f).expect("a covered file can be read");
        let lines: Vec<&str> = text.lines().collect();
        for (i, t) in unmarked(&lines) {
            bad.push(format!("{}:{}: {t}", rel(&f), i + 1));
        }
    }
    assert!(
        bad.is_empty(),
        "blocking calls without a `// sync-ok: <reason>` comment:\n{}",
        bad.join("\n")
    );
}

#[test]
fn every_sync_ok_has_a_reason() {
    let mut bad = Vec::new();
    for f in covered() {
        let text = std::fs::read_to_string(&f).expect("a covered file can be read");
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//!") || t.starts_with("///") {
                continue; // docs may mention the marker
            }
            if let Some(at) = line.find("sync-ok:") {
                let reason = line[at + "sync-ok:".len()..].trim();
                if reason.chars().count() < 8 {
                    bad.push(format!("{}:{}: {}", rel(&f), i + 1, line.trim()));
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

#[test]
fn the_scan_flags_an_unmarked_call_and_passes_a_marked_one() {
    let bad = ["fn f() {", "    let s = std::fs::read(p);", "}"];
    assert_eq!(unmarked(&bad).len(), 1);
    let ok = [
        "fn f() {",
        "    let s = std::fs::read(p); // sync-ok: one small file at start",
        "}",
    ];
    assert!(unmarked(&ok).is_empty());
    let above = [
        "fn f() {",
        "    // sync-ok: a user click, one small file",
        "    let s = std::fs::read(p);",
        "}",
    ];
    assert!(unmarked(&above).is_empty());
    let in_a_test = [
        "#[cfg(test)]",
        "mod t {",
        "    fn g() { std::fs::read(p); }",
        "}",
    ];
    assert!(unmarked(&in_a_test).is_empty());
    // A trait method's signature is a declaration, not a call.
    let signature = ["    fn sleep(&self, ms: u64) -> bool;"];
    assert!(unmarked(&signature).is_empty());
}
