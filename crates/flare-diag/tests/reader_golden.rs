//! M9 Task 23: the reader's output equals Prism Terminal's `tools/diag.mjs` on the same logs, case by
//! case (the goldens are Prism Terminal's own output, Task 2), and the `diag` binary runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use flare_diag::reader::{parse_args, read_lines, render, Utc};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn goldens() -> Vec<Value> {
    let text =
        std::fs::read_to_string(fixtures().join("reader_goldens.json")).expect("the goldens");
    serde_json::from_str(&text).expect("the goldens are JSON")
}

fn argv_of(case: &Value) -> Vec<String> {
    case["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|a| a.as_str().expect("an argument").to_owned())
        .collect()
}

#[test]
fn the_golden_outputs_equal_prism_terminals_diag_tool() {
    let cases = goldens();
    assert_eq!(cases.len(), 7);
    for case in &cases {
        let name = case["case"].as_str().expect("a case name");
        let argv = argv_of(case);
        let o = parse_args(&argv).unwrap_or_else(|e| panic!("{name}: {e}"));
        let label = o.dir.as_ref().expect("a dir").display().to_string();
        let lines = read_lines(&fixtures().join(&label));
        let out = render(&o, &label, &lines, 0, &Utc);
        assert_eq!(out, case["stdout"].as_str().expect("stdout"), "case {name}");
    }
}

#[test]
fn the_binary_runs() {
    let case = goldens()
        .into_iter()
        .find(|c| c["case"] == "n3")
        .expect("the n3 golden");
    let out = Command::new(env!("CARGO_BIN_EXE_diag"))
        .current_dir(fixtures())
        .args(["--dir", "reader/stall", "--utc", "--n", "3"])
        .output()
        .expect("the diag binary starts");
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        case["stdout"].as_str().expect("stdout")
    );
}
