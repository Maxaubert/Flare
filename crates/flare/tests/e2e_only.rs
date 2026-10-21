//! Scan (port of Prism Terminal's e2eOnly.test.ts): the test hook is reachable only from one place,
//! behind --e2e.

use std::path::{Path, PathBuf};

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

/// (file name, code before the first test module) for every source file.
fn sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files
        .iter()
        .map(|p| {
            // LF whatever the checkout's line endings are; a comment that names a call is no call.
            let text = std::fs::read_to_string(p).unwrap().replace("\r\n", "\n");
            let code = text
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, code)
        })
        .collect()
}

#[test]
fn the_hook_is_only_started_from_one_place_behind_the_flag() {
    let all = sources();
    for (name, code) in &all {
        if code.contains("e2e_hook::start") && name != "e2e_glue.rs" {
            panic!("{name} starts the hook; only e2e_glue.rs may");
        }
        if code.contains("start_with(") && name != "e2e_hook.rs" {
            panic!("{name} calls start_with; only e2e_hook.rs may");
        }
        if code.contains("register_ui_handler") && name != "e2e_glue.rs" && name != "e2e_hook.rs" {
            panic!("{name} registers a UI handler; only e2e_glue.rs may");
        }
        if code.contains("e2e_glue::install") && name != "window.rs" {
            panic!("{name} calls e2e_glue::install; only window.rs may");
        }
    }
    // start_with( is called in e2e_hook.rs only from fn start (the other hits are definitions).
    let hook = &all.iter().find(|(n, _)| n == "e2e_hook.rs").unwrap().1;
    let calls: Vec<&str> = hook
        .lines()
        .filter(|l| l.contains("start_with(") && !l.contains("fn start_with"))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "start_with is called more than once: {calls:?}"
    );
    assert!(calls[0].contains("crate::e2e::global().on()"), "{calls:?}");
    // install runs only inside an `if args.e2e` block of window.rs.
    let window = &all.iter().find(|(n, _)| n == "window.rs").unwrap().1;
    let at = window
        .find("e2e_glue::install")
        .expect("window.rs installs the hook");
    let before = &window[..at];
    let gate = before
        .rfind("if args.e2e")
        .expect("an `if args.e2e` block precedes the install call");
    assert!(
        !before[gate..].contains("\n    }\n"),
        "the `if args.e2e` block closed before the install call"
    );
}
