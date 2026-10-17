//! Scan: `flare-theme` is pure (Task 32). Outside test code it never touches the file system, a
//! process or a thread, so the maths and the resolvers run the same on every call and the UI thread
//! can call them on a click (rule 34). The crate also has no runtime dependency; `Cargo.toml` shows it.

use std::path::{Path, PathBuf};

const IMPURE: &[&str] = &[
    "std::fs",
    "std::process",
    "std::thread",
    "std::net",
    "std::env::var",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every impure path in the crate's code before its first `#[cfg(test)]`: (file, line no, text).
fn impure_lines() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    let mut hits = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("a source file can be read");
        let name = f.strip_prefix(&src).unwrap_or(&f).display().to_string();
        // `testutil.rs` is declared `#[cfg(test)] mod testutil;` in lib.rs: test code only.
        if name == "testutil.rs" {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("#[cfg(test)]") {
                break;
            }
            let code = line.split("//").next().unwrap_or("");
            if IMPURE.iter().any(|p| code.contains(p)) {
                hits.push(format!("{name}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    hits
}

#[test]
fn flare_theme_code_is_pure() {
    let hits = impure_lines();
    assert!(
        hits.is_empty(),
        "impure calls in flare-theme code (outside #[cfg(test)]):\n{}",
        hits.join("\n")
    );
}

#[test]
fn flare_theme_has_no_runtime_dependency() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("Cargo.toml can be read");
    let deps = manifest.split("[dependencies]").nth(1).unwrap_or("");
    let runtime = deps.split("[dev-dependencies]").next().unwrap_or("");
    assert!(
        runtime.lines().all(|l| l.trim().is_empty()),
        "flare-theme must have no runtime dependency:\n{runtime}"
    );
}
