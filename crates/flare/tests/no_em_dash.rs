//! Scan: no em-dash (U+2014) in any tracked text file (owner rule; the CI step does the same with git grep).

use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &["target", ".git", ".claude", ".e2e-cache", "node_modules"];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else if name != "Cargo.lock" {
            out.push(p);
        }
    }
}

#[test]
fn no_em_dash() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let dash = '\u{2014}';
    let mut hits = Vec::new();
    for f in files {
        // Binary files (PNG, ICO, fonts) do not decode as UTF-8 and are skipped.
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains(dash) {
                hits.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "em-dashes found (use en-dashes, commas or rephrase):\n{}",
        hits.join("\n")
    );
}
