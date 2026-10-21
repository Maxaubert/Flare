//! SelectionGate, selection_delete_allowed, selection_delete_keys, link_at (Prism Terminal's
//! termSelectionEdit.ts). A selection on the prompt line can be deleted with Backspace: the
//! caret walks to the end of the selection, then one Backspace per character is sent.
//! Positions are grid cells and char indices.

use crate::click_caret::{MouseTracking, arrow_keys, caret_delta};
use crate::links::find_links;

/// What the selection-delete rule needs to know about the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionGate {
    pub mouse_tracking: MouseTracking,
    pub alternate: bool,
    pub scrolled_back: bool,
    pub cursor_hidden: bool,
    pub text_below: bool,
    pub on_line: bool,
}

/// True when the selection lies on the line being edited, with no program owning the mouse,
/// no alternate screen, no scrolled-back view, a visible cursor and nothing printed below.
pub fn selection_delete_allowed(g: &SelectionGate) -> bool {
    g.mouse_tracking == MouseTracking::None
        && !g.alternate
        && !g.scrolled_back
        && !g.cursor_hidden
        && !g.text_below
        && g.on_line
}

/// The keys that delete the selection between cells `start` and `end` (either order): the caret
/// walks to its end, then one backspace per non-spacer cell. Nothing when the range is empty.
pub fn selection_delete_keys(
    widths: &[u8],
    cursor: usize,
    start: usize,
    end: usize,
    text_end: usize,
    application_cursor: bool,
) -> String {
    let to = start.max(end).min(text_end);
    let from = start.min(end);
    let count = (from..to)
        .filter(|&i| widths.get(i).copied().unwrap_or(1) != 0)
        .count();
    if count == 0 {
        return String::new();
    }
    let delta = caret_delta(widths, cursor, to, text_end);
    let mut keys = arrow_keys(delta, application_cursor);
    keys.push_str(&"\x7f".repeat(count));
    keys
}

/// The http(s) link covering char index `at` of the line's text, or None when `at` is off it
/// (a sentence's full stop after the link is not part of it).
pub fn link_at(text: &str, at: usize) -> Option<String> {
    find_links(text)
        .into_iter()
        .find(|l| l.start <= at && at < l.end)
        .map(|l| text.chars().skip(l.start).take(l.end - l.start).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: &str = "\x1b[D";
    const R: &str = "\x1b[C";
    const BS: &str = "\x7f";

    fn ones(n: usize) -> Vec<u8> {
        vec![1u8; n]
    }

    fn quiet() -> SelectionGate {
        SelectionGate {
            mouse_tracking: MouseTracking::None,
            alternate: false,
            scrolled_back: false,
            cursor_hidden: false,
            text_below: false,
            on_line: true,
        }
    }

    #[test]
    fn selection_delete_allowed_at_a_quiet_prompt() {
        assert!(selection_delete_allowed(&quiet()));
    }

    #[test]
    fn selection_delete_refuses_every_other_case() {
        let cases = [
            SelectionGate {
                on_line: false,
                ..quiet()
            },
            SelectionGate {
                alternate: true,
                ..quiet()
            },
            SelectionGate {
                scrolled_back: true,
                ..quiet()
            },
            SelectionGate {
                cursor_hidden: true,
                ..quiet()
            },
            SelectionGate {
                text_below: true,
                ..quiet()
            },
            SelectionGate {
                mouse_tracking: MouseTracking::Vt200,
                ..quiet()
            },
        ];
        for g in cases {
            assert!(!selection_delete_allowed(&g), "{g:?}");
        }
    }

    #[test]
    fn delete_keys_word_at_the_end() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 20, 15, 20, 20, false),
            BS.repeat(5)
        );
    }

    #[test]
    fn delete_keys_middle_selection() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 20, 10, 15, 20, false),
            format!("{}{}", L.repeat(5), BS.repeat(5))
        );
    }

    #[test]
    fn delete_keys_cursor_before_selection() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 4, 10, 15, 20, false),
            format!("{}{}", R.repeat(11), BS.repeat(5))
        );
    }

    #[test]
    fn delete_keys_backwards_selection() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 20, 15, 10, 20, false),
            format!("{}{}", L.repeat(5), BS.repeat(5))
        );
    }

    #[test]
    fn delete_keys_stop_at_the_text_end() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 20, 15, 35, 20, false),
            BS.repeat(5)
        );
        assert_eq!(selection_delete_keys(&line, 20, 25, 35, 20, false), "");
    }

    #[test]
    fn delete_keys_wide_counts_once() {
        let wide = [1u8, 1, 2, 0, 1];
        assert_eq!(
            selection_delete_keys(&wide, 5, 2, 4, 5, false),
            format!("{L}{BS}")
        );
    }

    #[test]
    fn delete_keys_application_cursor() {
        let line = ones(40);
        assert_eq!(
            selection_delete_keys(&line, 20, 10, 15, 20, true),
            format!("{}{}", "\x1bOD".repeat(5), BS.repeat(5))
        );
    }

    #[test]
    fn link_at_names_the_link() {
        let text = "see https://example.com/docs. then";
        assert_eq!(
            link_at(text, 4),
            Some("https://example.com/docs".to_string())
        );
        assert_eq!(
            link_at(text, 27),
            Some("https://example.com/docs".to_string())
        );
    }

    #[test]
    fn link_at_is_none_off_the_link() {
        let text = "see https://example.com/docs. then";
        assert_eq!(link_at(text, 2), None);
        assert_eq!(link_at(text, 28), None);
    }
}
