//! Reduced motion is one global (parity line 37): every animation duration in `ui/**/*.slint` goes
//! through `Motion.d()`, so `Motion.reduced` stills the whole app (PT index.css: 0.01 ms, one iteration).

use std::path::{Path, PathBuf};

fn slint_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("ui folder") {
        // sync-ok: test code only, never the app
        let path = entry.expect("entry").path(); // sync-ok: test code only, never the app
        if path.is_dir() {
            slint_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("slint") {
            out.push(path);
        }
    }
}

/// Every `duration:` / `delay:` value inside an `animate` block, with its file and line.
fn animate_values(src: &str) -> Vec<(usize, String, String)> {
    let mut found = Vec::new();
    let mut in_block = false;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("animate ") {
            in_block = true;
        }
        if in_block {
            for key in ["duration:", "delay:"] {
                if let Some(at) = t.find(key) {
                    let rest = t[at + key.len()..].trim();
                    let value = rest.split(';').next().unwrap_or("").trim();
                    found.push((i + 1, key.to_string(), value.to_string()));
                }
            }
            if t.contains('}') {
                in_block = false;
            }
        }
    }
    found
}

#[test]
fn no_animate_block_bypasses_the_motion_global() {
    let mut files = Vec::new();
    slint_files(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/ui")),
        &mut files,
    );
    assert!(files.len() > 10, "the ui folder is scanned");
    let mut bypass = Vec::new();
    let mut seen = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).expect("slint file"); // sync-ok: test code only, never the app
        for (line, key, value) in animate_values(&src) {
            seen += 1;
            // A delay of a bare zero is nothing to still.
            if key == "delay:" && value == "0ms" {
                continue;
            }
            if !value.contains("Motion.d(") {
                bypass.push(format!(
                    "{}:{line}: {key} {value}",
                    f.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    assert!(seen >= 25, "the scan finds the animate blocks ({seen})");
    assert!(
        bypass.is_empty(),
        "animations that skip Motion.d(): {bypass:#?}"
    );
}
