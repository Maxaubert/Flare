//! Test support: reads the oracle's JSON fixtures (Task 2).

// Unused until Task 2 adds the fixtures; Task 2 removes this allow.
#[allow(dead_code)]
pub(crate) fn fixture(name: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}
