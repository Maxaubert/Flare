//! M9 Task 6: grouping of the release notes under headings.
//!
//! The patch notes, sorted under headings and worded for a reader. Ported from Prism Terminal
//! v0.55.6 `releaseGroups.ts`. The parser (`notes`) has already made plain strings; this module
//! only files each one under a heading, takes off the commit-type prefix and the trailing pull
//! request number, and starts the line with a capital. It never turns text into markup.
//!
//! Pure: no window, no network, no disk.

use std::sync::LazyLock;

use regex::Regex;

use crate::notes::ReleaseNotes;

/// The four headings, in the order they are shown: what is new to use first, what nobody needs to
/// read last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReleaseKind {
    Features,
    Improvements,
    Fixes,
    Internal,
}

/// One heading and its lines. Never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSection {
    pub kind: ReleaseKind,
    pub heading: &'static str,
    pub entries: Vec<String>,
}

const SECTIONS: [(ReleaseKind, &str); 4] = [
    (ReleaseKind::Features, "New features"),
    (ReleaseKind::Improvements, "Improvements"),
    (ReleaseKind::Fixes, "Bug fixes"),
    (ReleaseKind::Internal, "Under the hood"),
];

fn slot(k: ReleaseKind) -> usize {
    match k {
        ReleaseKind::Features => 0,
        ReleaseKind::Improvements => 1,
        ReleaseKind::Fixes => 2,
        ReleaseKind::Internal => 3,
    }
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a fixed pattern")
}

// A conventional-commit prefix: `fix:`, `feat(terminal):`, `refactor(core)!:`. Written with
// `[a-zA-Z]`, not `[a-z]` under `(?i)`: Unicode case folding would let a Kelvin sign pass as `k`.
static CONVENTIONAL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^([a-zA-Z]{2,10})(?:\([^()]{0,40}\))?!?:\s+"));
// One or more pull request numbers at the very end: "(#27) (#29)".
static TRAILING_REFS: LazyLock<Regex> = LazyLock::new(|| re(r"(?:\s*\(#[0-9]{1,9}\))+\s*$"));
// Titles with no prefix that still say what they are. "fixes", "fixed" and "fix" each need a
// space or a colon after them, which the TypeScript checks with a lookahead: "Fixed-width font
// option" is not a fix.
static READS_AS_FIX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:fixes|fixed|fix|bug ?fix|hotfix)[\s:]"));
// The same word used as a LABEL ("Fix: the prompt", "Fixes - the prompt"). The dash is an en-dash.
static FIX_LABEL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:fixes|fixed|fix|bug ?fix|hotfix)(?::|\s+[-\x{2013}]\s)\s*"));
// Dependabot's bumps. (Prism Terminal's own pin-bump line is not ported: Flare has no such bot.)
static READS_AS_INTERNAL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:bump\s+\S+\s+from\s|update\s+dependenc)"));

fn kind_of(word: &str) -> Option<ReleaseKind> {
    match word.to_ascii_lowercase().as_str() {
        "feat" | "feature" => Some(ReleaseKind::Features),
        "fix" | "bugfix" | "hotfix" => Some(ReleaseKind::Fixes),
        "perf" | "refactor" | "style" | "ui" => Some(ReleaseKind::Improvements),
        "docs" | "doc" | "chore" | "build" | "ci" | "test" | "tests" | "deps" | "revert" => {
            Some(ReleaseKind::Internal)
        }
        _ => None,
    }
}

fn capitalised(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Where one entry goes, and how it reads there. The window uses [`group_release_notes`]; this is
/// exported for the oracle.
pub fn classify_entry(entry: &str) -> (ReleaseKind, String) {
    let mut text = TRAILING_REFS.replacen(entry, 1, "").trim().to_string();
    let mut kind = ReleaseKind::Features;
    // Only a prefix that IS a commit type is taken off: "Phone: a cleaner tab drawer" names the
    // part of the app, and that is worth reading.
    let prefix = CONVENTIONAL
        .captures(&text)
        .and_then(|c| kind_of(&c[1]).map(|k| (k, c[0].len())));
    if let Some((k, len)) = prefix {
        kind = k;
        text = text[len..].to_string();
    } else if READS_AS_INTERNAL.is_match(&text) {
        kind = ReleaseKind::Internal;
    } else if READS_AS_FIX.is_match(&text) {
        kind = ReleaseKind::Fixes;
        // "Fix the prompt" keeps its verb; "Fix: the prompt" and "Fixes - x" lose the label, which
        // the heading now says.
        text = FIX_LABEL.replacen(&text, 1, "").into_owned();
    }
    let out = capitalised(text.trim());
    // A title that was only a prefix and a number has nothing left to show; the entry as it came
    // is better than an empty line.
    (
        kind,
        if out.is_empty() {
            entry.to_string()
        } else {
            out
        },
    )
}

/// The sections, in order, empty ones left out. A body with no notes has none: the window prints
/// the parser's stand-in line instead.
pub fn group_release_notes(notes: &ReleaseNotes) -> Vec<ReleaseSection> {
    if notes.empty {
        return Vec::new();
    }
    let mut buckets: [Vec<String>; 4] = Default::default();
    for entry in &notes.entries {
        let (kind, text) = classify_entry(entry);
        buckets[slot(kind)].push(text);
    }
    SECTIONS
        .iter()
        .zip(buckets)
        .filter(|(_, entries)| !entries.is_empty())
        .map(|((kind, heading), entries)| ReleaseSection {
            kind: *kind,
            heading,
            entries,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::parse_release_notes;
    use crate::preview::SAMPLE_NOTES;

    #[test]
    fn reads_a_conventional_prefix_and_takes_it_off() {
        assert_eq!(
            classify_entry("fix(terminal): a prompt survives a resize (#31)"),
            (ReleaseKind::Fixes, "A prompt survives a resize".to_string())
        );
        assert_eq!(
            classify_entry("feat: dictation"),
            (ReleaseKind::Features, "Dictation".to_string())
        );
        assert_eq!(
            classify_entry("perf(search): one collator, not five thousand").0,
            ReleaseKind::Improvements
        );
        assert_eq!(
            classify_entry("refactor(core)!: the host seam"),
            (ReleaseKind::Improvements, "The host seam".to_string())
        );
        for p in ["docs", "chore", "build", "ci", "test", "revert"] {
            assert_eq!(
                classify_entry(&format!("{p}: something")).0,
                ReleaseKind::Internal
            );
        }
    }

    #[test]
    fn calls_an_unmarked_title_a_feature() {
        assert_eq!(
            classify_entry("Dictation: speak into the terminal (#22)"),
            (
                ReleaseKind::Features,
                "Dictation: speak into the terminal".to_string()
            )
        );
    }

    #[test]
    fn keeps_a_prefix_that_names_a_part_of_the_app() {
        assert_eq!(
            classify_entry("Phone: a cleaner tab drawer").1,
            "Phone: a cleaner tab drawer"
        );
        assert_eq!(
            classify_entry("Command help: a searchable popup").0,
            ReleaseKind::Features
        );
    }

    #[test]
    fn reads_an_unprefixed_fix_as_one() {
        assert_eq!(
            classify_entry("Fix the prompt after a resize"),
            (
                ReleaseKind::Fixes,
                "Fix the prompt after a resize".to_string()
            )
        );
        assert_eq!(
            classify_entry("Fixes: the prompt after a resize").1,
            "The prompt after a resize"
        );
        assert_eq!(
            classify_entry("Fixed - the prompt after a resize").1,
            "The prompt after a resize"
        );
    }

    #[test]
    fn is_not_fooled_by_a_word_that_only_starts_like_one() {
        assert_eq!(
            classify_entry("Fixed-width font option"),
            (ReleaseKind::Features, "Fixed-width font option".to_string())
        );
        assert_eq!(
            classify_entry("Fixture loading is faster").0,
            ReleaseKind::Features
        );
    }

    #[test]
    fn files_the_bots_under_the_hood() {
        assert_eq!(
            classify_entry("Bump electron from 43.1.0 to 43.1.1 (#34)"),
            (
                ReleaseKind::Internal,
                "Bump electron from 43.1.0 to 43.1.1".to_string()
            )
        );
    }

    #[test]
    fn drops_every_pull_request_number_at_the_end_and_only_at_the_end() {
        assert_eq!(
            classify_entry("Window edges setting (#27) (#29)").1,
            "Window edges setting"
        );
        assert_eq!(
            classify_entry("Closes (#12) properly").1,
            "Closes (#12) properly"
        );
    }

    #[test]
    fn never_returns_an_empty_line() {
        assert_eq!(classify_entry("fix: (#9)").1, "fix: (#9)");
        assert_eq!(classify_entry("(#9)").1, "(#9)");
    }

    #[test]
    fn sorts_the_preview_body_under_headings_in_reading_order() {
        let notes = parse_release_notes(Some(SAMPLE_NOTES));
        let sections = group_release_notes(&notes);
        let headings: Vec<&str> = sections.iter().map(|s| s.heading).collect();
        assert_eq!(
            headings,
            vec!["New features", "Bug fixes", "Under the hood"]
        );
        assert_eq!(
            sections[0].entries[0],
            "The update button opens a window with the patch notes, Cancel and Install"
        );
        assert_eq!(
            sections[1].entries,
            vec![
                "A prompt survives the window getting narrower and wider again".to_string(),
                "Closing a tab that hosts an agent asks first, working or idle".to_string(),
            ]
        );
        assert_eq!(
            sections[2].entries,
            vec!["Bump electron from 43.1.0 to 43.1.1".to_string()]
        );
        let all: Vec<&String> = sections.iter().flat_map(|s| &s.entries).collect();
        // Nothing is lost on the way, and nothing still reads like `git log`.
        assert_eq!(all.len(), notes.entries.len());
        for line in all {
            assert!(!line.is_empty());
            assert!(!ends_with_pr_ref(line), "{line:?}");
            assert_eq!(
                line.chars()
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>()),
                line.chars().next().map(String::from)
            );
        }
    }

    #[test]
    fn leaves_an_empty_section_out_and_keeps_the_order_within_one() {
        let sections = group_release_notes(&parse_release_notes(Some("* fix: b\n* fix: a")));
        assert_eq!(
            sections,
            vec![ReleaseSection {
                kind: ReleaseKind::Fixes,
                heading: "Bug fixes",
                entries: vec!["B".to_string(), "A".to_string()],
            }]
        );
    }

    #[test]
    fn has_nothing_to_say_about_a_body_with_no_notes() {
        assert!(group_release_notes(&parse_release_notes(None)).is_empty());
        assert!(group_release_notes(&parse_release_notes(Some(""))).is_empty());
    }

    #[test]
    fn only_moves_strings_about() {
        let sections = group_release_notes(&parse_release_notes(Some(
            "* fix: <img src=x onerror=alert(1)> x",
        )));
        for s in &sections {
            for e in &s.entries {
                assert!(!has_tag_open(e), "{e:?}");
            }
        }
    }

    fn ends_with_pr_ref(s: &str) -> bool {
        let t = s.trim_end();
        t.ends_with(')') && t.rfind("(#").is_some()
    }

    fn has_tag_open(s: &str) -> bool {
        let b = s.as_bytes();
        b.windows(2)
            .any(|w| w[0] == b'<' && w[1].is_ascii_alphabetic())
    }
}
