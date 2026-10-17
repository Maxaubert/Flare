//! Scan: the agent code and its plugin carry no Prism name (rule (b), the M6 plan's Task 35).
//!
//! Flare is not Prism Terminal. A source line may name Prism only in two shapes: a doc line that
//! names Prism Terminal as the source (the trimmed line starts with `//` and contains
//! `Prism Terminal`), and a test literal that proves Prism's words are REFUSED (the line
//! contains `"prism-agent` or `"prism-terminal-status`). Any other line that mentions Prism, and
//! any em-dash, fails.

use std::path::{Path, PathBuf};

const EM_DASH: char = '\u{2014}';

/// True when the line breaks the name rule: it mentions Prism outside the two allowed shapes.
fn offends(line: &str) -> bool {
    if line.contains(EM_DASH) {
        return true;
    }
    if !line.to_ascii_lowercase().contains("prism") {
        return false;
    }
    let trimmed = line.trim_start();
    let source_note = trimmed.starts_with("//") && trimmed.contains("Prism Terminal");
    let refusal_literal =
        line.contains("\"prism-agent") || line.contains("\"prism-terminal-status");
    !(source_note || refusal_literal)
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/flare-agent sits two levels under the repository root")
        .to_path_buf()
}

/// Every file the name rule covers, relative to the repository root.
fn covered_files() -> Vec<PathBuf> {
    let root = root();
    let mut files = Vec::new();
    collect(&root.join("crates/flare-agent/src"), &mut files);
    collect(&root.join("crates/flare-agent/claude-plugin"), &mut files);
    for single in [
        "crates/flare-diag/src/agent_diag.rs",
        "crates/flare-settings/src/agent_prefs.rs",
        "crates/flare-win/src/procs.rs",
        "crates/flare-win/src/taskbar.rs",
        "crates/flare-win/src/motion.rs",
        "crates/flare/ui/tabmark.slint",
        "crates/flare/ui/skeleton.slint",
    ] {
        files.push(root.join(single));
    }
    files.sort();
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn no_agent_file_names_prism_outside_the_two_allowed_shapes() {
    let root = root();
    let mut bad = Vec::new();
    for f in covered_files() {
        let text = std::fs::read_to_string(&f)
            .unwrap_or_else(|e| panic!("{} can be read: {e}", f.display()));
        for (i, line) in text.lines().enumerate() {
            if offends(line) {
                let rel = f.strip_prefix(&root).unwrap_or(&f);
                bad.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "Prism names or em-dashes in Flare's agent code:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_scan_rejects_a_prism_name_in_code() {
    assert!(offends(r#"let x = "prism";"#));
    assert!(offends("// the agent is Prism's own"));
    assert!(offends("let dash = \"a\u{2014}b\";"));
}

#[test]
fn the_scan_allows_the_two_shapes_only() {
    assert!(!offends(
        "//! The poll follows Prism Terminal's agentPoll.ts."
    ));
    assert!(!offends(
        "    // Prism Terminal's agentDetect, ported as is."
    ));
    assert!(!offends(
        r#"assert!(!accepts("prism-agent:state=working"));"#
    ));
    assert!(!offends(r#"assert!(!accepts("prism-terminal-status"));"#));
    assert!(!offends(r#"let a = "flare-agent";"#));
}
