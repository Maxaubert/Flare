//! The quiet context (rule 1268): what the person marked in the side panel, handed to the tab's Claude at its next
//! prompt and never typed. Its `UserPromptSubmit` hook (`flare-mcp --hook-context`) asks `panel.state` with
//! `"context": true`, and the answer gains `context`: the text selected in the panel right now (a file's lines, or a
//! page's text) and the person's drawn arrow while Flare keeps it. The hook words it so the agent uses it only when the
//! person's message is about it.
//!
//! Here the pieces are made as data: the text is gated (nothing that prints invisibly or is a control, line breaks
//! kept) and cut to [`SELECTION_MAX`] characters with `cut` set. A page's title and address are its own words, so the
//! hook quotes them between its page markers; a file's name is Flare's.

use serde_json::{Value, json};

/// The longest selection handed on (characters); a longer one is cut and says so.
pub const SELECTION_MAX: usize = 2000;
/// The longest page title or file name handed on (characters).
const NAME_MAX: usize = 300;
/// The longest page address handed on (characters).
const ADDRESS_MAX: usize = 2048;

/// Where a selection or a mark is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place<'a> {
    /// A file in the panel (its path, or the panel's title for text with no file), with the selection's lines
    /// (`lines 2-3`) when they are known.
    File {
        name: &'a str,
        lines: Option<&'a str>,
    },
    /// A plain web page: the address and the title its own document gave (untrusted).
    Page { title: &'a str, url: &'a str },
    /// A page Flare made (a mini-app or a plugin's page): its address means nothing to an agent.
    Other,
}

/// `s` on one line: every control and every kind of space or line break is a space, at most `max` characters.
fn one_line(s: &str, max: usize) -> String {
    crate::ask_agent::gated(s)
        .chars()
        .map(|c| {
            if c.is_control() || c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect()
}

/// The place's fields of a selection or a mark.
fn place_fields(place: Place<'_>, v: &mut Value) {
    match place {
        Place::File { name, lines } => {
            v["file"] = json!(one_line(name, NAME_MAX));
            if let Some(lines) = lines {
                v["lines"] = json!(one_line(lines, NAME_MAX));
            }
        }
        Place::Page { title, url } => {
            v["page"] = json!({
                "title": one_line(title, NAME_MAX),
                "url": one_line(&crate::ask_agent::without_userinfo(url), ADDRESS_MAX),
            });
        }
        Place::Other => v["other"] = json!(true),
    }
}

/// The selection as the hook gets it: `{text, cut}` and its place. The text is gated with its line breaks kept (a
/// Windows or Unicode break becomes `\n`), the blank lines around it trimmed, and cut to [`SELECTION_MAX`] characters.
/// None when it holds nothing that prints.
pub fn selection(text: &str, place: Place<'_>) -> Option<Value> {
    let clean = crate::ask_agent::gated(text)
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let body = clean.trim_matches('\n');
    if body.trim().is_empty() {
        return None;
    }
    let cut = body.chars().count() > SELECTION_MAX;
    let kept: String = body.chars().take(SELECTION_MAX).collect();
    let mut v = json!({"text": kept, "cut": cut});
    place_fields(place, &mut v);
    Some(v)
}

/// The person's drawn arrow as the hook gets it: `{kind: "arrow"}` and its place. What it points at is page content, so
/// it is not handed on: the agent looks with `browser_look`.
pub fn arrow(place: Place<'_>) -> Value {
    let mut v = json!({"kind": "arrow"});
    place_fields(place, &mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_selection_keeps_its_lines_and_names_the_file() {
        let v = selection(
            "fn main() {\r\n    run();\r\n}\n",
            Place::File {
                name: "C:\\src\\main.rs",
                lines: Some("lines 4-6"),
            },
        )
        .unwrap();
        assert_eq!(v["text"], "fn main() {\n    run();\n}");
        assert_eq!(v["cut"], false);
        assert_eq!(v["file"], "C:\\src\\main.rs");
        assert_eq!(v["lines"], "lines 4-6");
        assert!(v.get("page").is_none());
    }

    #[test]
    fn a_long_selection_is_cut_at_2000_characters_and_says_so() {
        let place = Place::Other;
        let v = selection(&"\u{e9}".repeat(SELECTION_MAX), place).unwrap();
        assert_eq!(v["cut"], false, "exactly the cap is not cut");
        let v = selection(&"\u{e9}".repeat(SELECTION_MAX + 1), place).unwrap();
        assert_eq!(v["cut"], true);
        assert_eq!(v["text"].as_str().unwrap().chars().count(), SELECTION_MAX);
        assert_eq!(v["other"], true);
    }

    #[test]
    fn a_selection_of_nothing_that_prints_is_none() {
        for blank in ["", " \n\t\r\n", "\u{200b}\u{202e}", "\u{1b}\u{7}"] {
            assert_eq!(selection(blank, Place::Other), None, "{blank:?}");
        }
    }

    #[test]
    fn page_text_loses_what_prints_invisibly_and_every_control_but_the_line_break() {
        let v = selection(
            "a\u{1b}[201~\u{3}b\u{200b}\u{2028}c\u{9b}",
            Place::Page {
                title: "T\nitle\u{202e}",
                url: "https://me:pw@example.com/x",
            },
        )
        .unwrap();
        assert_eq!(v["text"], "a[201~b\nc");
        assert_eq!(v["page"]["title"], "T itle");
        assert_eq!(v["page"]["url"], "https://example.com/x");
    }

    #[test]
    fn an_arrow_names_where_it_is_and_never_what_it_points_at() {
        let v = arrow(Place::Page {
            title: "Docs",
            url: "https://a.example/",
        });
        assert_eq!(
            v,
            json!({"kind": "arrow", "page": {"title": "Docs", "url": "https://a.example/"}})
        );
        let v = arrow(Place::File {
            name: "C:\\notes.md",
            lines: None,
        });
        assert_eq!(v, json!({"kind": "arrow", "file": "C:\\notes.md"}));
    }

    #[test]
    fn a_long_title_and_address_are_capped() {
        let long = format!("https://a.example/?q={}", "z".repeat(5000));
        let v = arrow(Place::Page {
            title: &"t".repeat(1000),
            url: &long,
        });
        assert_eq!(
            v["page"]["title"].as_str().unwrap().chars().count(),
            NAME_MAX
        );
        assert_eq!(
            v["page"]["url"].as_str().unwrap().chars().count(),
            ADDRESS_MAX
        );
    }
}
