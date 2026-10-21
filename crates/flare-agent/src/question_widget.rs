//! Claude Code's question widget, read off the screen (Task 6). Ported from Prism Terminal's
//! `questionWidget.ts` and `shared/questionShown.ts` (Sonara #323).
//!
//! Claude Code shows the questions of one AskUserQuestion one at a time and tells plugins nothing until
//! all are answered. The screen can: under the tab row (`←  ☐ Colour  ☒ Size  ✔ Submit  →`) stands the
//! full text of the question showing, a long one wrapped over lines that start with `│`. After the last
//! question the widget reads "Review your answers".

use regex::Regex;
use std::sync::LazyLock;

/// What the question widget shows: a question's text, or the review of the answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShownQuestion {
    /// The question's text, trimmed.
    Text(String),
    /// The review of the answers after the last question.
    Review,
}

/// The longest question text accepted, in UTF-16 units (the same cap as the TS bridge).
const MAX_TEXT_UNITS: usize = 2000;

impl ShownQuestion {
    /// A question text checked the way the bridge checks it: the raw text at most 2000 UTF-16 units and
    /// non-blank once trimmed. Returns the trimmed text.
    pub fn checked_text(raw: &str) -> Option<ShownQuestion> {
        if raw.encode_utf16().count() > MAX_TEXT_UNITS {
            return None;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(ShownQuestion::Text(trimmed.to_string()))
        }
    }
}

/// The tab row: the arrows around the tabs, the last one Submit.
static TAB_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*←.*✔\s*Submit\s*→\s*$").expect("TAB_ROW"));
/// The first option ends the question: `❯ 1. Red`, `  2. [ ] Linux`.
static OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:❯\s*)?[0-9]+\.\s").expect("OPTION"));
/// The widget's last line while a question shows.
static HINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*Enter to select").expect("HINT"));
/// The review screen's heading.
static REVIEW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*Review your answers\s*$").expect("REVIEW"));

/// The question the widget shows on these screen rows (top to bottom), or None when no question widget
/// is on them. The LAST tab row is the live widget: a resize can leave a stale frame of it above.
pub fn extract_question(rows: &[String]) -> Option<ShownQuestion> {
    let tab = rows.iter().rposition(|r| TAB_ROW.is_match(r))?;
    let rest = &rows[tab + 1..];
    if rest.iter().any(|r| REVIEW.is_match(r)) {
        return Some(ShownQuestion::Review);
    }
    if !rest.iter().any(|r| HINT.is_match(r)) {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for row in rest {
        if OPTION.is_match(row) {
            break;
        }
        let stripped = strip_frame(row);
        if !stripped.is_empty() {
            lines.push(stripped);
        }
    }
    let text = lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        None
    } else {
        Some(ShownQuestion::Text(text))
    }
}

/// A row with its leading `│` frame (and one space after it) removed, and trimmed.
fn strip_frame(row: &str) -> String {
    let trimmed_start = row.trim_start();
    let inner = match trimmed_start.strip_prefix('│') {
        Some(after) => after.strip_prefix(char::is_whitespace).unwrap_or(after),
        None => row,
    };
    inner.trim().to_string()
}

/// Two extracts are the same question, or both the review. Both absent counts as the same.
pub fn same_question(a: Option<&ShownQuestion>, b: Option<&ShownQuestion>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(ShownQuestion::Review), Some(ShownQuestion::Review)) => true,
        (Some(ShownQuestion::Text(x)), Some(ShownQuestion::Text(y))) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// A named screen from the fixture the PT source measured (`question_widget_screens.json`).
    fn at(name: &str) -> Vec<String> {
        let all: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/question_widget_screens.json"
        ))
        .expect("fixture parses");
        all[name]
            .as_array()
            .expect("fixture key is an array")
            .iter()
            .map(|v| v.as_str().expect("row is a string").to_string())
            .collect()
    }

    fn text(s: &str) -> Option<ShownQuestion> {
        Some(ShownQuestion::Text(s.to_string()))
    }

    #[test]
    fn reads_the_question_under_the_tab_row() {
        assert_eq!(
            extract_question(&at("run2-01-question-1")),
            text("Pick a colour?")
        );
        assert_eq!(extract_question(&at("run2-02-right")), text("Pick a size?"));
    }

    #[test]
    fn moves_on_when_an_answer_moves_the_widget_on() {
        assert_eq!(
            extract_question(&at("run2-07-enter-answer")),
            text("Pick a shape?")
        );
    }

    #[test]
    fn joins_a_wrapped_question_with_single_spaces() {
        assert_eq!(
            extract_question(&at("run3-01-question-1")),
            text(
                "When the installer finds the search service already running from an earlier version, should it stop the service, replace its files and start it again, or leave the running copy alone until the next reboot?"
            )
        );
    }

    #[test]
    fn reads_a_multi_select_question_ticked_or_not() {
        assert_eq!(
            extract_question(&at("run3-02-right")),
            text("Which platforms should the build cover?")
        );
        assert_eq!(
            extract_question(&at("run3-03-space-on-windows")),
            text("Which platforms should the build cover?")
        );
    }

    #[test]
    fn keeps_the_question_while_an_answer_is_typed() {
        assert_eq!(
            extract_question(&at("run3-07-type")),
            text("Name the release?")
        );
    }

    #[test]
    fn reports_the_review_of_the_answers() {
        assert_eq!(
            extract_question(&at("run2-08-enter-answer")),
            Some(ShownQuestion::Review)
        );
    }

    #[test]
    fn reads_the_last_widget_frame_when_a_stale_one_stands_above_it() {
        let mut both = at("run2-01-question-1");
        both.extend(at("run2-02-right"));
        assert_eq!(extract_question(&both), text("Pick a size?"));
    }

    #[test]
    fn is_none_without_the_widget() {
        let plain = vec![
            "PS C:\\> claude".to_string(),
            String::new(),
            "Some reply text.".to_string(),
        ];
        assert_eq!(extract_question(&plain), None);
        assert_eq!(extract_question(&[]), None);
    }

    #[test]
    fn a_shown_question_is_checked() {
        assert_eq!(ShownQuestion::checked_text("  hi  "), text("hi"));
        assert_eq!(ShownQuestion::checked_text("   "), None);
        assert_eq!(ShownQuestion::checked_text(&"a".repeat(2001)), None);
        assert_eq!(
            ShownQuestion::checked_text(&"a".repeat(2000)),
            text(&"a".repeat(2000))
        );
    }

    #[test]
    fn same_question_compares_texts_and_reviews() {
        let a = ShownQuestion::Text("x".into());
        let b = ShownQuestion::Text("x".into());
        assert!(same_question(None, None));
        assert!(!same_question(Some(&a), None));
        assert!(same_question(Some(&a), Some(&b)));
        assert!(same_question(
            Some(&ShownQuestion::Review),
            Some(&ShownQuestion::Review)
        ));
        assert!(!same_question(Some(&ShownQuestion::Review), Some(&a)));
    }
}
