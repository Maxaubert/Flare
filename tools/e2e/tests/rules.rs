//! Harness rules that a source scan holds (M10 Task 74).

use std::fs;
use std::path::PathBuf;

fn scenario_file(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("scenarios")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Settings rows are reached by their id (through the row-to-page table), never by a grid index.
#[test]
fn settings_rows_are_reached_by_id() {
    const FILES: [&str; 4] = [
        "settings_a.rs",
        "settings_b.rs",
        "settings_c.rs",
        "settings_d.rs",
    ];
    const BY_INDEX: [&str; 4] = ["rows[", "row_at(", "row_index(", "nth("];
    for name in FILES {
        let src = scenario_file(name);
        // A probe or a click names its row by id: `(app, "row-id")`, or the id a helper was given.
        let by_id = src.contains("(app, \"") || src.contains("(app, id)");
        assert!(by_id, "{name} reaches no settings row by its id");
        for (n, line) in src.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for needle in BY_INDEX {
                assert!(
                    !code.contains(needle),
                    "{name}:{} reaches a settings row by index ({needle}): {line}",
                    n + 1
                );
            }
        }
    }
}
