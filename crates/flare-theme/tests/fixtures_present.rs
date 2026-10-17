//! The oracle fixtures are present and have Prism Terminal's counts (Task 2).

use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> serde_json::Value {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} does not parse: {e}"))
}

const TWENTY_SEVEN: [&str; 27] = [
    "presets.json",
    "resolved_presets.json",
    "on_grounds.json",
    "custom_opaque.json",
    "chrome_presets.json",
    "chrome_alpha_sweep.json",
    "wall_order.json",
    "link_colours.json",
    "sidebar_grounds.json",
    "chrome_token_names.json",
    "retired.json",
    "fonts.json",
    "ansi_vectors.json",
    "parse_vectors.json",
    "legible_vectors.json",
    "selection_vectors.json",
    "commit_vectors.json",
    "on_ground_vectors.json",
    "custom_alpha_vectors.json",
    "mark_vectors.json",
    "oklab_vectors.json",
    "edge_vectors.json",
    "chrome_edges.json",
    "chrome_picks.json",
    "chrome_levels.json",
    "agent_colours.json",
    "marks_apart.json",
];

#[test]
fn all_twenty_seven_fixtures_exist_and_parse() {
    for name in TWENTY_SEVEN {
        let _ = fixture(name);
    }
}

#[test]
fn fixture_counts_are_prism_terminals() {
    assert_eq!(fixture("presets.json").as_array().unwrap().len(), 39);
    assert_eq!(fixture("fonts.json").as_array().unwrap().len(), 15);
    assert_eq!(fixture("retired.json").as_object().unwrap().len(), 31);
    assert_eq!(
        fixture("chrome_token_names.json").as_array().unwrap().len(),
        31
    );
    assert_eq!(fixture("wall_order.json").as_array().unwrap().len(), 39);
    assert_eq!(fixture("ansi_vectors.json").as_array().unwrap().len(), 200);
    assert_eq!(fixture("parse_vectors.json").as_array().unwrap().len(), 28);
}

#[test]
fn generated_presets_have_39_entries_and_31_retired_ids() {
    let src = include_str!("../src/presets_data.rs");
    assert_eq!(src.matches("Preset {").count(), 39);
    let retired_lines = src.lines().filter(|l| l.starts_with("    (\"")).count();
    assert_eq!(retired_lines, 31);
}
