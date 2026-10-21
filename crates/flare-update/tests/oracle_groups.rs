//! M9 Task 6: the grouping against Prism Terminal's own answers (`groups.json`).
//!
//! The fixture holds 150 release bodies with the sections Prism Terminal's `releaseGroups.ts`
//! made of them, and 40 titles with their kind and text. Two of the titles are Prism Terminal's
//! own pin-bump lines (`Terminal core: prism-term-core ...`); Flare has no such bot, so the rule
//! that files them under the hood is not ported and those two carry `"flare": "skip"`.

use flare_update::groups::{ReleaseKind, classify_entry, group_release_notes};
use flare_update::notes::parse_release_notes;
use serde_json::Value;

/// The body as Prism Terminal's fixture writes it: a string, or `{concat: [...]}` / `{repeat:
/// [unit, n]}` for a long one (the same encoding `oracle_notes.rs` reads). Anything else is no body.
fn body_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            if let Some(Value::Array(parts)) = o.get("concat") {
                Some(
                    parts
                        .iter()
                        .map(|p| body_of(p).unwrap_or_default())
                        .collect(),
                )
            } else if let Some(Value::Array(r)) = o.get("repeat") {
                let unit = r.first().and_then(Value::as_str).unwrap_or_default();
                let n = r.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
                Some(unit.repeat(n))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn kind_name(k: ReleaseKind) -> &'static str {
    match k {
        ReleaseKind::Features => "features",
        ReleaseKind::Improvements => "improvements",
        ReleaseKind::Fixes => "fixes",
        ReleaseKind::Internal => "internal",
    }
}

#[test]
fn the_oracle_agrees_on_150_bodies_and_40_entries() {
    let v: Value = serde_json::from_str(include_str!("fixtures/groups.json")).expect("groups.json");

    let bodies = v["bodies"].as_array().expect("bodies");
    assert_eq!(bodies.len(), 150);
    for (i, b) in bodies.iter().enumerate() {
        let body = body_of(&b["body"]);
        let got: Vec<(String, String, Vec<String>)> =
            group_release_notes(&parse_release_notes(body.as_deref()))
                .iter()
                .map(|s| {
                    (
                        kind_name(s.kind).to_string(),
                        s.heading.to_string(),
                        s.entries.clone(),
                    )
                })
                .collect();
        let want: Vec<(String, String, Vec<String>)> = b["sections"]
            .as_array()
            .expect("sections")
            .iter()
            .map(|s| {
                (
                    s["kind"].as_str().unwrap().to_string(),
                    s["heading"].as_str().unwrap().to_string(),
                    s["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| e.as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(got, want, "sections of body {i}");
    }

    let entries = v["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 40);
    let mut checked = 0;
    for e in entries {
        if e.get("flare").and_then(Value::as_str) == Some("skip") {
            continue;
        }
        let input = e["entry"].as_str().unwrap();
        let (kind, text) = classify_entry(input);
        assert_eq!(
            kind_name(kind),
            e["kind"].as_str().unwrap(),
            "kind of {input:?}"
        );
        assert_eq!(text, e["text"].as_str().unwrap(), "text of {input:?}");
        checked += 1;
    }
    assert_eq!(checked, 38);
}
