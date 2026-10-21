//! M9 Task 5: the release-notes parser against Prism Terminal's own answers (`notes.json`).

use std::time::Instant;

use flare_update::notes::{
    MAX_BODY_CHARS, MAX_ENTRIES, MAX_ENTRY_CHARS, NO_NOTES, cut_utf16, more_line,
    parse_release_notes, utf16_len,
};
use serde_json::Value;

struct Case {
    name: Option<String>,
    body: Option<String>,
    entries: Vec<String>,
    more: usize,
    empty: bool,
    lens: Vec<usize>,
}

fn build(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            if let Some(Value::Array(parts)) = o.get("concat") {
                Some(parts.iter().map(|p| build(p).unwrap_or_default()).collect())
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

fn cases() -> Vec<Case> {
    let raw = include_str!("fixtures/notes.json");
    let v: Value = serde_json::from_str(raw).expect("notes.json");
    v.as_array()
        .expect("an array")
        .iter()
        .map(|c| Case {
            name: c.get("name").and_then(Value::as_str).map(str::to_string),
            body: build(&c["body"]),
            entries: c["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect(),
            more: c["more"].as_u64().unwrap() as usize,
            empty: c["empty"].as_bool().unwrap(),
            lens: c["lens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap() as usize)
                .collect(),
        })
        .collect()
}

fn check(c: &Case) {
    let got = parse_release_notes(c.body.as_deref());
    let label = c.name.clone().unwrap_or_else(|| "(unnamed)".into());
    assert_eq!(got.entries, c.entries, "entries of {label}");
    assert_eq!(got.more, c.more, "more of {label}");
    assert_eq!(got.empty, c.empty, "empty of {label}");
    let lens: Vec<usize> = got.entries.iter().map(|e| utf16_len(e)).collect();
    assert_eq!(lens, c.lens, "UTF-16 lengths of {label}");
}

/// Every named case whose name is the PT test title, or the title with a " (a)" style suffix.
fn named(title: &str) {
    let mut found = 0;
    for c in cases() {
        if let Some(n) = &c.name
            && (n == title || n.starts_with(&format!("{title} (")))
        {
            check(&c);
            found += 1;
        }
    }
    assert!(found > 0, "no oracle case is named {title:?}");
}

macro_rules! pt_test {
    ($f:ident, $title:expr) => {
        #[test]
        fn $f() {
            named($title);
        }
    };
}

pt_test!(
    is_one_plain_entry_per_pull_request_in_order,
    "is one plain entry per pull request, in order"
);
pt_test!(
    drops_the_author_and_the_url_and_keeps_the_pull_request_number_as_text,
    "drops the author and the url, and keeps the pull request number as text"
);
pt_test!(
    does_not_print_a_number_the_title_already_carries,
    "does not print a number the title already carries"
);
pt_test!(
    drops_the_heading_the_new_contributors_and_the_full_changelog,
    "drops the heading, the new contributors and the full changelog"
);
pt_test!(
    reads_windows_line_endings_the_same_way,
    "reads Windows line endings the same way"
);
pt_test!(
    takes_a_section_after_new_contributors_back_up,
    "takes a section that follows New Contributors back up"
);
pt_test!(
    strips_an_author_with_no_url_after_it,
    "strips an author with no url after it"
);
pt_test!(
    keeps_a_title_that_names_a_handle_and_then_says_in,
    "keeps a title that names a handle and then says \"in\": only a url ends an author tail"
);
pt_test!(
    keeps_a_by_that_is_part_of_the_title,
    "keeps a \"by\" that is part of the title"
);
pt_test!(
    reads_dashes_pluses_numbers_and_task_boxes_as_bullets,
    "reads dashes, pluses, numbers and task boxes as bullets"
);
pt_test!(
    reads_a_plain_paragraph_line_as_an_entry,
    "reads a plain paragraph line as an entry"
);
pt_test!(
    reads_a_wrapped_paragraph_as_one_entry_and_a_bullet_ends_it,
    "reads a paragraph wrapped over several lines as ONE entry, and a bullet ends it"
);
pt_test!(
    joins_an_indented_continuation_onto_the_bullet_above_it,
    "joins an indented continuation onto the bullet above it"
);
pt_test!(
    drops_headings_and_rules_whatever_they_say,
    "drops headings and rules, whatever they say"
);
pt_test!(collapses_whitespace, "collapses whitespace");
pt_test!(
    turns_a_markdown_link_into_its_text_never_its_target,
    "turns a markdown link into its text, never its target"
);
pt_test!(
    turns_an_image_into_its_alt_text,
    "turns an image into its alt text"
);
pt_test!(
    strips_html_tags_and_comments_alike,
    "strips html, tags and comments alike"
);
pt_test!(
    is_not_fooled_by_a_tag_hidden_inside_a_tag,
    "is not fooled by a tag hidden inside a tag"
);
pt_test!(
    leaves_a_lone_angle_bracket_alone,
    "leaves a lone angle bracket alone: it is only text"
);
pt_test!(
    strips_emphasis_marks_and_code_ticks_and_keeps_snake_case,
    "strips emphasis marks and code ticks, and keeps snake_case"
);
pt_test!(
    strips_control_characters_and_the_marks_that_reorder_text,
    "strips control characters and the marks that reorder text"
);
pt_test!(
    never_returns_anything_but_strings_and_never_an_empty_one,
    "never returns anything but strings, and never an empty one"
);
pt_test!(
    cuts_a_long_entry_with_an_ellipsis_to_the_cap_exactly,
    "cuts a long entry, with an ellipsis, to the cap exactly"
);
pt_test!(
    keeps_the_pull_request_number_when_it_cuts_the_title,
    "keeps the pull request number when it cuts the title"
);
pt_test!(
    leaves_an_entry_of_exactly_the_cap_alone,
    "leaves an entry of exactly the cap alone"
);
pt_test!(
    shows_the_first_entries_and_counts_the_rest,
    "shows the first entries and counts the rest"
);
pt_test!(
    counts_nothing_when_the_list_fits_exactly,
    "counts nothing when the list fits exactly"
);

#[test]
fn the_each_table_at_line_102_row_by_row() {
    let mut rows = 0;
    for c in cases() {
        if c.name
            .as_deref()
            .is_some_and(|n| n.ends_with(" is one sensible line"))
        {
            check(&c);
            assert_eq!(c.entries, vec![NO_NOTES.to_string()]);
            rows += 1;
        }
    }
    assert_eq!(rows, 8);
}

#[test]
fn reads_only_the_head_of_an_enormous_body_and_quickly() {
    let body = "* line\n".repeat(400_000);
    assert!(body.len() > MAX_BODY_CHARS);
    let t0 = Instant::now();
    let notes = parse_release_notes(Some(&body));
    assert!(t0.elapsed().as_millis() < 250);
    assert_eq!(notes.entries.len(), MAX_ENTRIES);
    assert!(notes.more < MAX_BODY_CHARS / "* line\n".len());
}

#[test]
fn an_enormous_body_is_read_quickly() {
    let body = "* line of a change by @someone in https://x.test/pull/9\n".repeat(100_000);
    assert!(body.len() > 5_000_000);
    let t0 = Instant::now();
    let notes = parse_release_notes(Some(&body));
    assert!(t0.elapsed().as_millis() < 250, "took {:?}", t0.elapsed());
    assert_eq!(notes.entries.len(), MAX_ENTRIES);
}

#[test]
fn does_not_hang_on_a_line_built_to_make_a_regex_backtrack() {
    let line = format!(
        "* {} by @{} in {}",
        "[a](".repeat(3000),
        "a-".repeat(3000),
        "<".repeat(3000)
    );
    let t0 = Instant::now();
    let _ = parse_release_notes(Some(&line));
    assert!(t0.elapsed().as_millis() < 250, "took {:?}", t0.elapsed());
}

#[test]
fn says_how_many_were_left_out() {
    assert_eq!(more_line(1), "+ 1 more");
    assert_eq!(more_line(12), "+ 12 more");
}

#[test]
fn the_oracle_agrees_on_all_450_bodies() {
    let all = cases();
    assert_eq!(all.len(), 450);
    for c in &all {
        check(c);
    }
}

#[test]
fn a_title_ending_on_a_surrogate_pair_is_cut_before_the_pair() {
    // 158 letters, then an emoji (two UTF-16 units) across the cut at 159 units.
    let body = format!("* {}\u{1F600}{}", "a".repeat(158), "b".repeat(10));
    let notes = parse_release_notes(Some(&body));
    assert_eq!(notes.entries, vec![format!("{}\u{2026}", "a".repeat(158))]);
    assert!(utf16_len(&notes.entries[0]) <= MAX_ENTRY_CHARS);
    // The helper itself: the pair is dropped whole, never split.
    assert_eq!(cut_utf16("ab\u{1F600}cd", 4), "ab\u{2026}");
    assert_eq!(cut_utf16("abc", 3), "abc");
    assert_eq!(utf16_len("\u{1F600}"), 2);
}

#[test]
fn never_contains_html_markup_or_a_control_character() {
    for c in cases() {
        let got = parse_release_notes(c.body.as_deref());
        for e in &got.entries {
            assert!(!e.contains("<script"), "{e:?}");
            assert!(!e.contains('\u{202e}'), "{e:?}");
            assert!(!e.contains('\u{200b}'), "{e:?}");
        }
    }
}

#[test]
fn a_non_string_body_is_no_notes() {
    let n = parse_release_notes(None);
    assert_eq!(n.entries, vec![NO_NOTES.to_string()]);
    assert_eq!(n.more, 0);
    assert!(n.empty);
}
