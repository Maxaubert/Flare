//! The Flare hotkeys sheet (rule 1262): every key Flare answers, in three groups, as Settings > Terminal > Keys shows
//! them behind the info button of the "Flare hotkeys" row. Built from the key tables (the chord table's `key_label_on`,
//! the prompt keys' `prompt_key_label`, the terminal's paste and new-line labels), per system, never typed here: a new
//! chord needs a row here, and a test checks every chord of the table is on the sheet.

use flare_term::keys::newline_label;
use flare_term::prompt_keys::{MAC_DELETE_LINE_LABEL, PromptKey as P, prompt_key_label};

use crate::keymap::{ChordId, KeyOs, chord, copy_paste_labels, key_label_on};

/// The sheet's groups, in order.
pub const TYPING: &str = "Typing to an agent";
pub const TABS: &str = "Tabs";
pub const APP: &str = "App";

/// One row of the sheet: its group, what it does, and its keys as shown. `labels` are the table labels the keys come
/// from (the search also reads them, so "left" finds a row shown with an arrow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyRow {
    pub group: &'static str,
    pub action: &'static str,
    pub keys: String,
    pub labels: Vec<&'static str>,
}

/// Where a row's keys come from.
enum Src {
    Prompt(P),
    /// Two prompt keys, one row ("Shift+←→").
    Pair(P, P),
    /// A chord: its own key.
    Chord(ChordId),
    /// A chord with an agent key too: the agent key first, then its own.
    Both(ChordId),
    Paste,
    Newline,
    /// The Mac's Cmd+Backspace (no row on Windows).
    MacDeleteLine,
}

/// The sheet, in order: the mockup's rows first in each group, then the rest of the table.
const ROWS: &[(&str, &str, Src)] = &[
    (TYPING, "Select all", Src::Prompt(P::SelectAll)),
    (
        TYPING,
        "Select a letter",
        Src::Pair(P::SelectLeft, P::SelectRight),
    ),
    (
        TYPING,
        "Select a word",
        Src::Pair(P::SelectWordLeft, P::SelectWordRight),
    ),
    (
        TYPING,
        "Select to start or end",
        Src::Pair(P::SelectHome, P::SelectEnd),
    ),
    (TYPING, "Jump a word", Src::Pair(P::WordLeft, P::WordRight)),
    (TYPING, "Go to start or end", Src::Pair(P::Home, P::End)),
    (
        TYPING,
        "Delete a word",
        Src::Pair(P::DeleteWordBack, P::DeleteWordForward),
    ),
    (TYPING, "Delete to the line's start", Src::MacDeleteLine),
    (TYPING, "Cut", Src::Prompt(P::Cut)),
    (TYPING, "Undo", Src::Prompt(P::Undo)),
    (TYPING, "Paste a screenshot", Src::Paste),
    (TYPING, "New line", Src::Newline),
    (
        TYPING,
        "Previous or next message",
        Src::Pair(P::PrevMessage, P::NextMessage),
    ),
    (TYPING, "Clear the prompt", Src::Prompt(P::ClearPrompt)),
    (TABS, "Next tab", Src::Both(ChordId::NextTab)),
    (TABS, "Sidebar", Src::Both(ChordId::Sidebar)),
    (TABS, "Reopen closed tab", Src::Chord(ChordId::ReopenTab)),
    (TABS, "New tab in a folder", Src::Chord(ChordId::NewTabIn)),
    (TABS, "New tab", Src::Chord(ChordId::NewTab)),
    (TABS, "Close tab", Src::Chord(ChordId::CloseTab)),
    (TABS, "Go to tab 1 to 9", Src::Chord(ChordId::JumpTab)),
    (TABS, "Find a tab", Src::Chord(ChordId::FindTab)),
    (APP, "Tab on a machine", Src::Chord(ChordId::RemoteTab)),
    (APP, "Browser panel", Src::Chord(ChordId::Browser)),
    (APP, "Open the folder", Src::Chord(ChordId::OpenFolder)),
    (APP, "Show or hide the panel", Src::Chord(ChordId::Panel)),
    (
        APP,
        "Keys to the panel and back",
        Src::Chord(ChordId::PanelFocus),
    ),
    (APP, "Find", Src::Chord(ChordId::Find)),
    (APP, "Settings", Src::Chord(ChordId::Settings)),
    (APP, "Command help", Src::Chord(ChordId::Help)),
    (APP, "Full screen", Src::Chord(ChordId::Fullscreen)),
    (APP, "Window menu", Src::Chord(ChordId::SystemMenu)),
    (APP, "Zoom in", Src::Chord(ChordId::ZoomIn)),
    (APP, "Zoom out", Src::Chord(ChordId::ZoomOut)),
    (APP, "Actual size", Src::Chord(ChordId::ZoomReset)),
];

/// A label split into its modifiers and an arrow: "Shift+Left" is ("Shift+", '←'), "⇧←" is ("⇧", '←').
fn arrow_split(l: &str) -> Option<(&str, char)> {
    const ARROWS: [(&str, char); 4] = [
        ("Left", '\u{2190}'),
        ("Up", '\u{2191}'),
        ("Right", '\u{2192}'),
        ("Down", '\u{2193}'),
    ];
    for (word, glyph) in ARROWS {
        if let Some(p) = l.strip_suffix(word) {
            return Some((p, glyph));
        }
        if let Some(p) = l.strip_suffix(glyph) {
            return Some((p, glyph));
        }
    }
    None
}

/// Two keys on one row: arrows with the same modifiers share them ("Shift+←→"), anything else is a list.
pub fn pair_text(a: &str, b: &str) -> String {
    match (arrow_split(a), arrow_split(b)) {
        (Some((pa, ga)), Some((pb, gb))) if pa == pb => format!("{pa}{ga}{gb}"),
        _ => format!("{a}, {b}"),
    }
}

/// The table labels of one row on this system; empty when the system has no such key.
fn labels(os: KeyOs, src: &Src) -> Vec<&'static str> {
    let c = |id| key_label_on(os, id, false);
    let v = match src {
        Src::Prompt(k) => vec![prompt_key_label(os, *k)],
        Src::Pair(a, b) => vec![prompt_key_label(os, *a), prompt_key_label(os, *b)],
        Src::Chord(id) => vec![c(*id)],
        Src::Both(id) => {
            let mut v = Vec::new();
            if chord(*id).agent.is_some() {
                v.push(key_label_on(os, *id, true));
            }
            v.push(c(*id));
            v
        }
        Src::Paste => vec![copy_paste_labels(os).1],
        Src::Newline => vec![newline_label(os)],
        Src::MacDeleteLine => {
            if os.is_mac() {
                vec![MAC_DELETE_LINE_LABEL]
            } else {
                Vec::new()
            }
        }
    };
    v.into_iter().filter(|l| !l.is_empty()).collect()
}

/// Every row of the sheet on this system, in order. A key the system lacks (the Window menu on a Mac, the zoom keys on
/// Windows) has no row.
pub fn hotkey_rows(os: KeyOs) -> Vec<HotkeyRow> {
    ROWS.iter()
        .filter_map(|&(group, action, ref src)| {
            let labels = labels(os, src);
            let keys = match (src, labels.as_slice()) {
                (_, []) => return None,
                (Src::Pair(..), [a, b]) => pair_text(a, b),
                _ => labels.join(", "),
            };
            Some(HotkeyRow {
                group,
                action,
                keys,
                labels,
            })
        })
        .collect()
}

/// The rows the search keeps: every row while it is empty, else those whose action, keys or table labels hold it,
/// ignoring case and the spaces around it.
pub fn filter_rows(rows: &[HotkeyRow], query: &str) -> Vec<HotkeyRow> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| {
            r.action.to_lowercase().contains(&q)
                || r.keys.to_lowercase().contains(&q)
                || r.labels.iter().any(|l| l.to_lowercase().contains(&q))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{MAC_ONLY, ORDER};
    use flare_term::prompt_keys::PROMPT_KEYS;

    fn all_labels(rows: &[HotkeyRow]) -> Vec<&'static str> {
        rows.iter().flat_map(|r| r.labels.iter().copied()).collect()
    }

    #[test]
    fn every_chord_of_the_table_is_on_the_sheet_by_its_label() {
        for os in [KeyOs::Windows, KeyOs::Mac] {
            let rows = hotkey_rows(os);
            let labels = all_labels(&rows);
            for id in ORDER.into_iter().chain(MAC_ONLY) {
                // The agent guard is no action: it keeps Ctrl+C and Ctrl+D from an agent (its own row in Settings).
                if id == ChordId::AgentGuard {
                    continue;
                }
                let own = key_label_on(os, id, false);
                if !own.is_empty() {
                    assert!(labels.contains(&own), "{os:?} {id:?} {own}");
                }
                if chord(id).agent.is_some() {
                    let agent = key_label_on(os, id, true);
                    assert!(labels.contains(&agent), "{os:?} {id:?} agent {agent}");
                }
            }
        }
    }

    #[test]
    fn every_prompt_key_paste_and_new_line_are_on_the_sheet() {
        for os in [KeyOs::Windows, KeyOs::Mac] {
            let labels = all_labels(&hotkey_rows(os));
            for k in PROMPT_KEYS {
                assert!(labels.contains(&prompt_key_label(os, k)), "{os:?} {k:?}");
            }
            assert!(labels.contains(&copy_paste_labels(os).1));
            assert!(labels.contains(&newline_label(os)));
            assert_eq!(
                labels.contains(&MAC_DELETE_LINE_LABEL),
                os.is_mac(),
                "Cmd+Backspace is the Mac's alone"
            );
        }
    }

    #[test]
    fn the_windows_sheet_reads_as_the_mockup() {
        let rows = hotkey_rows(KeyOs::Windows);
        let find = |a: &str| {
            rows.iter()
                .find(|r| r.action == a)
                .unwrap_or_else(|| panic!("{a}"))
                .clone()
        };
        let want = [
            ("Select all", "Ctrl+A", TYPING),
            ("Select a letter", "Shift+\u{2190}\u{2192}", TYPING),
            ("Select a word", "Ctrl+Shift+\u{2190}\u{2192}", TYPING),
            ("Select to start or end", "Shift+Home, Shift+End", TYPING),
            ("Jump a word", "Ctrl+\u{2190}\u{2192}", TYPING),
            ("Delete a word", "Ctrl+Backspace, Ctrl+Delete", TYPING),
            ("Cut", "Ctrl+X", TYPING),
            ("Undo", "Ctrl+Z", TYPING),
            ("Paste a screenshot", "Ctrl+V", TYPING),
            ("New line", "Shift+Enter", TYPING),
            (
                "Previous or next message",
                "Ctrl+Shift+\u{2191}\u{2193}",
                TYPING,
            ),
            ("Clear the prompt", "Ctrl+Shift+Backspace", TYPING),
            ("Next tab", "Ctrl+PgDn, Ctrl+Tab", TABS),
            ("Sidebar", "Ctrl+S, Ctrl+Shift+B", TABS),
            ("Reopen closed tab", "Ctrl+Shift+T", TABS),
            ("New tab in a folder", "Ctrl+Shift+N", TABS),
            ("Tab on a machine", "Ctrl+Shift+R", APP),
            ("Browser panel", "Ctrl+Shift+L", APP),
            ("Open the folder", "Ctrl+Shift+E", APP),
        ];
        for (action, keys, group) in want {
            let r = find(action);
            assert_eq!(r.keys, keys, "{action}");
            assert_eq!(r.group, group, "{action}");
        }
        // The groups come in order, each in one run.
        let groups: Vec<&str> = rows.iter().map(|r| r.group).collect();
        let mut runs = groups.clone();
        runs.dedup();
        assert_eq!(runs, [TYPING, TABS, APP]);
        // Windows has no zoom keys and no Cmd+Backspace row; the Mac has no Window menu.
        assert!(rows.iter().all(|r| r.action != "Zoom in"));
        assert!(
            rows.iter()
                .all(|r| r.action != "Delete to the line's start")
        );
        assert!(
            hotkey_rows(KeyOs::Mac)
                .iter()
                .all(|r| r.action != "Window menu")
        );
        for r in &rows {
            assert!(!r.keys.contains('\u{2014}') && !r.action.contains('\u{2014}'));
        }
    }

    #[test]
    fn the_mac_sheet_names_cmd_and_option_in_their_glyphs() {
        let rows = hotkey_rows(KeyOs::Mac);
        let keys = |a: &str| rows.iter().find(|r| r.action == a).unwrap().keys.clone();
        assert_eq!(keys("Select all"), "\u{2318}A");
        assert_eq!(keys("Select a word"), "\u{2325}\u{21e7}\u{2190}\u{2192}");
        assert_eq!(keys("Reopen closed tab"), "\u{21e7}\u{2318}T");
        assert_eq!(keys("Paste a screenshot"), "\u{2318}V");
        assert_eq!(keys("Delete to the line's start"), "\u{2318}\u{232b}");
        assert_eq!(keys("Zoom in"), "\u{2318}+");
    }

    #[test]
    fn two_keys_share_their_modifiers_only_when_both_are_arrows() {
        assert_eq!(
            pair_text("Shift+Left", "Shift+Right"),
            "Shift+\u{2190}\u{2192}"
        );
        assert_eq!(
            pair_text("\u{21e7}\u{2190}", "\u{21e7}\u{2192}"),
            "\u{21e7}\u{2190}\u{2192}"
        );
        assert_eq!(pair_text("Home", "End"), "Home, End");
        assert_eq!(
            pair_text("Ctrl+Left", "Shift+Right"),
            "Ctrl+Left, Shift+Right"
        );
    }

    #[test]
    fn the_search_reads_actions_keys_and_labels_and_ignores_case() {
        let rows = hotkey_rows(KeyOs::Windows);
        assert_eq!(filter_rows(&rows, "").len(), rows.len());
        assert_eq!(filter_rows(&rows, "   ").len(), rows.len());
        let tab: Vec<&str> = filter_rows(&rows, "TAB").iter().map(|r| r.action).collect();
        assert!(tab.contains(&"Next tab") && tab.contains(&"Tab on a machine"));
        assert!(!tab.contains(&"Undo"));
        let by_key: Vec<&str> = filter_rows(&rows, "ctrl+shift+t")
            .iter()
            .map(|r| r.action)
            .collect();
        assert_eq!(by_key, ["Reopen closed tab"]);
        // "left" finds the arrow rows through their table labels.
        assert!(
            filter_rows(&rows, "left")
                .iter()
                .any(|r| r.action == "Select a letter")
        );
        assert!(filter_rows(&rows, "nothing like this").is_empty());
    }
}
