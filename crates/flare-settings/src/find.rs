//! Find a setting: the index and its search (M7 Task 11).
//!
//! Source: Prism Terminal v0.55.6's `core/renderer/settings/layout/search.ts` (the search),
//! `coreIndex.ts` and `components/settings/settingsIndex.ts` (the index), and their tests.
//! Every word of the query must match the START of a word of the entry's label, subtext,
//! section, page name or hidden keywords, case and accents folded. An entry whose LABEL holds
//! every word comes first, an exact label before all; the rest keep the index order.

use crate::pages::{PAGES, page_of_section};
use crate::rows::{ROW_ORDER, RowDef, row};
use crate::sections::section_title;
use unicode_normalization::UnicodeNormalization;

/// One row the index offers. `section` is the heading the result shows ("" when the section
/// has none); `section_id` is the section the flash falls back to.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub id: String,
    pub page: String,
    pub view: Option<String>,
    pub section: String,
    pub section_id: Option<String>,
    pub label: String,
    pub sub: String,
    pub icon: String,
    pub keywords: Option<String>,
}

/// The page NAMES, by page id (the index knows a page only by its id).
pub type PageNames = std::collections::HashMap<String, String>;

/// What this PC draws: the GPU row needs an NVIDIA card, every dictation row needs the
/// Dictation page to be wired (decision D6; M8 sets both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEnv {
    pub nvidia: bool,
    pub dictation_wired: bool,
}

/// Lower case, accents gone: NFD, then the combining marks U+0300 to U+036F removed, then
/// lower case (the same range the TypeScript regex removes).
pub fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .collect::<String>()
        .to_lowercase()
}

/// The words of a text: folded, split on every run outside `a` to `z` and `0` to `9`, empties
/// dropped.
pub fn words(text: &str) -> Vec<String> {
    let folded = fold(text);
    folded
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The page names of the rail, by id, as the results show them.
pub fn page_names() -> PageNames {
    PAGES
        .iter()
        .map(|page| (page.id.to_string(), page.label.to_string()))
        .collect()
}

/// The entries that match `query`, best first. A blank query finds nothing.
pub fn search_settings<'a>(
    index: &'a [IndexEntry],
    query: &str,
    pages: &PageNames,
) -> Vec<&'a IndexEntry> {
    let q = words(query);
    if q.is_empty() {
        return Vec::new();
    }
    let wanted = fold(query).split_whitespace().collect::<Vec<_>>().join(" ");
    let mut scored: Vec<(u8, usize, &'a IndexEntry)> = Vec::new();
    for (i, e) in index.iter().enumerate() {
        let page = pages
            .get(&e.page)
            .map(String::as_str)
            .unwrap_or(e.page.as_str());
        let hay = words(
            &[
                e.label.as_str(),
                e.sub.as_str(),
                e.section.as_str(),
                page,
                e.keywords.as_deref().unwrap_or(""),
            ]
            .join(" "),
        );
        if !q
            .iter()
            .all(|w| hay.iter().any(|h| h.starts_with(w.as_str())))
        {
            continue;
        }
        let label = words(&e.label);
        let in_label = q
            .iter()
            .all(|w| label.iter().any(|h| h.starts_with(w.as_str())));
        let exact = fold(&e.label).trim() == wanted;
        let rank = if exact {
            0
        } else if in_label {
            1
        } else {
            2
        };
        scored.push((rank, i, e));
    }
    scored.sort_by_key(|(rank, i, _)| (*rank, *i));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

/// Whether a row is drawn on this PC: the GPU row only with NVIDIA, the dictation rows only on
/// a wired Dictation page (decision D6).
fn drawn(row: &RowDef, env: &IndexEnv) -> bool {
    if row.only_nvidia && !env.nvidia {
        return false;
    }
    if row.id.starts_with("dictation-") && !env.dictation_wired {
        return false;
    }
    true
}

fn entry_of(row: &RowDef) -> IndexEntry {
    IndexEntry {
        id: row.id.to_string(),
        page: page_of_section(row.section).unwrap_or(row.page).to_string(),
        view: None,
        section: section_title(row.section).unwrap_or("").to_string(),
        section_id: Some(row.section.to_string()),
        label: row.label.to_string(),
        sub: row.sub.to_string(),
        icon: row.icon.to_string(),
        keywords: Some(row.keywords.to_string()),
    }
}

/// The index of every row drawn on this PC, in `ROW_ORDER`.
pub fn settings_index(env: &IndexEnv) -> Vec<IndexEntry> {
    ROW_ORDER
        .iter()
        .filter_map(|id| row(id))
        .filter(|r| drawn(r, env))
        .map(entry_of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str, sub: &str) -> IndexEntry {
        IndexEntry {
            id: id.to_string(),
            page: "appearance".to_string(),
            view: None,
            section: "Theme".to_string(),
            section_id: None,
            label: label.to_string(),
            sub: sub.to_string(),
            icon: "brush".to_string(),
            keywords: None,
        }
    }

    fn with(mut e: IndexEntry, page: &str, section: &str, keywords: Option<&str>) -> IndexEntry {
        e.page = page.to_string();
        e.section = section.to_string();
        e.keywords = keywords.map(str::to_string);
        e
    }

    /// PT's six-entry test index (search.test.ts), verbatim.
    fn test_index() -> Vec<IndexEntry> {
        vec![
            with(
                entry(
                    "term-theme",
                    "Terminal theme",
                    "Colours of the terminal and the window.",
                ),
                "appearance",
                "Theme",
                Some("palette scheme"),
            ),
            entry(
                "window-background",
                "Background colour",
                "Behind the text in window and terminal.",
            ),
            entry(
                "window-accent",
                "Accent colour",
                "Highlights, the active tab and selection.",
            ),
            with(
                entry(
                    "term-font",
                    "Terminal text size",
                    "Text size for every terminal.",
                ),
                "terminal",
                "Text",
                None,
            ),
            with(
                entry("agent-color", "Agent working colour", "Follows the accent."),
                "agents",
                "Mark colours",
                None,
            ),
            with(
                entry(
                    "dictation-language",
                    "Spoken language",
                    "The language you speak.",
                ),
                "dictation",
                "Listening",
                Some("norwegian"),
            ),
        ]
    }

    fn names() -> PageNames {
        [("appearance", "Appearance"), ("agents", "Agents")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ids(query: &str) -> Vec<String> {
        let index = test_index();
        search_settings(&index, query, &names())
            .iter()
            .map(|r| r.id.clone())
            .collect()
    }

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_index_has_44_rows_without_nvidia() {
        // 32 rows outside dictation, 8 dictation rows and From any app (the GPU row needs NVIDIA).
        let n = settings_index(&IndexEnv {
            nvidia: false,
            dictation_wired: true,
        })
        .len();
        assert_eq!(n, 44);
        assert!(
            settings_index(&IndexEnv {
                nvidia: false,
                dictation_wired: true,
            })
            .iter()
            .any(|e| e.id == "dictation-anywhere" && e.section == "From any app")
        );
    }

    fn index_all() -> Vec<IndexEntry> {
        settings_index(&IndexEnv {
            nvidia: true,
            dictation_wired: true,
        })
    }

    #[test]
    fn empty_or_blank_query_finds_nothing() {
        assert!(ids("").is_empty());
        assert!(ids("   ").is_empty());
    }

    #[test]
    fn every_word_must_match() {
        assert_eq!(ids("accent colour"), s(&["window-accent", "agent-color"]));
        assert!(ids("accent spoken").is_empty());
    }

    #[test]
    fn matches_the_start_of_a_word_not_the_middle() {
        assert!(ids("col").contains(&"window-background".to_string()));
        assert!(ids("olour").is_empty());
    }

    #[test]
    fn folds_case_and_accents() {
        assert_eq!(fold("Àccént"), "accent");
        assert_eq!(ids("ACCÉNT"), s(&["window-accent", "agent-color"]));
    }

    #[test]
    fn a_label_match_comes_first_an_exact_label_first_of_all() {
        // "terminal" is in the theme's label and in the background's subtext.
        assert_eq!(
            ids("terminal"),
            s(&["term-theme", "term-font", "window-background"])
        );
        assert_eq!(ids("terminal text size")[0], "term-font");
        assert_eq!(ids("Agent working colour")[0], "agent-color");
    }

    #[test]
    fn reads_hidden_keywords_the_section_and_the_page() {
        assert_eq!(ids("palette"), s(&["term-theme"]));
        assert_eq!(ids("norwegian"), s(&["dictation-language"]));
        assert_eq!(ids("mark colours"), s(&["agent-color"]));
        assert_eq!(ids("agents"), s(&["agent-color"]));
    }

    #[test]
    fn nothing_found_is_an_empty_list() {
        assert!(ids("zebra").is_empty());
    }

    #[test]
    fn find_words_the_see_through_row_as_the_page_does() {
        let row = index_all()
            .into_iter()
            .find(|e| e.id == "term-acrylic")
            .expect("term-acrylic is indexed");
        assert_eq!(row.label, "See through window");
        assert_eq!(
            row.sub,
            "How much of the desktop shows behind every surface."
        );
        assert!(row.keywords.unwrap_or_default().contains("see through"));
    }

    #[test]
    fn the_index_orders_every_row_once_core_and_own() {
        let ids: Vec<String> = index_all().into_iter().map(|e| e.id).collect();
        let order: Vec<String> = ROW_ORDER.iter().map(|id| id.to_string()).collect();
        assert_eq!(ids, order);
    }

    #[test]
    fn the_gpu_row_is_indexed_only_with_nvidia() {
        let ids: Vec<String> = settings_index(&IndexEnv {
            nvidia: false,
            dictation_wired: true,
        })
        .into_iter()
        .map(|e| e.id)
        .collect();
        let expect: Vec<String> = ROW_ORDER
            .iter()
            .filter(|id| **id != "dictation-gpu")
            .map(|id| id.to_string())
            .collect();
        assert_eq!(ids, expect);
    }

    #[test]
    fn dictation_rows_are_indexed_only_when_the_page_is_wired() {
        let count = |nvidia: bool, wired: bool| {
            settings_index(&IndexEnv {
                nvidia,
                dictation_wired: wired,
            })
            .iter()
            .filter(|e| e.id.starts_with("dictation-"))
            .count()
        };
        assert_eq!(count(true, false), 0);
        assert_eq!(count(false, false), 0);
        // Eight dictation rows, From any app (N12, N31), and the GPU row with NVIDIA.
        assert_eq!(count(false, true), 9);
        assert_eq!(count(true, true), 10);
    }

    #[test]
    fn rows_say_where_they_live() {
        let index = index_all();
        let at = |id: &str| {
            let e = index
                .iter()
                .find(|e| e.id == id)
                .unwrap_or_else(|| panic!("{id} not indexed"));
            format!("{}/{}", e.page, e.section)
        };
        assert_eq!(at("term-theme"), "appearance/Theme");
        assert_eq!(at("window-accent"), "appearance/Theme");
        assert_eq!(at("term-shell"), "terminal/Shell");
        assert_eq!(at("taskbar-badge"), "agents/Tab marks");
        assert_eq!(at("agent-rainbow"), "agents/Tab marks");
        assert_eq!(at("agent-hooks"), "agents/Claude Code");
        assert_eq!(at("dictation-enabled"), "dictation/");
        assert_eq!(at("app-version"), "about/");
        assert_eq!(at("diag-verbose"), "diagnostics/");
    }

    #[test]
    fn there_is_no_tab_style_row_and_rainbow_follows_finished() {
        let ids: Vec<String> = index_all().into_iter().map(|e| e.id).collect();
        assert!(!ids.contains(&"tab-style".to_string()));
        let done = ids
            .iter()
            .position(|id| id == "agent-done-on")
            .expect("done row");
        assert_eq!(
            ids.iter().position(|id| id == "agent-rainbow"),
            Some(done + 1)
        );
    }

    #[test]
    fn tab_switching_and_tab_position_are_found_by_what_they_do() {
        let index = index_all();
        let pos = |id: &str| index.iter().position(|e| e.id == id).expect("indexed");
        assert_eq!(pos("tab-switch"), pos("tab-layout") + 1);
        let row = &index[pos("tab-switch")];
        assert_eq!(format!("{}/{}", row.page, row.section), "appearance/Window");
        let kw = row.keywords.clone().unwrap_or_default();
        for word in ["recent", "ctrl", "mru", "order"] {
            assert!(kw.contains(word), "tab-switch keywords lack {word}");
        }
        let layout = &index[pos("tab-layout")];
        assert_eq!(
            format!("{}/{}", layout.page, layout.section),
            "appearance/Window"
        );
        assert_eq!(pos("tab-layout"), 0);
        let kw = layout.keywords.clone().unwrap_or_default();
        for word in ["vertical", "sidebar"] {
            assert!(kw.contains(word), "tab-layout keywords lack {word}");
        }
    }

    #[test]
    fn every_indexed_label_finds_its_own_row_first() {
        let index = index_all();
        let pages = page_names();
        for e in &index {
            let found = search_settings(&index, &e.label, &pages);
            assert_eq!(
                found.first().map(|r| r.id.as_str()),
                Some(e.id.as_str()),
                "label {:?} does not find its own row first",
                e.label
            );
        }
    }

    #[test]
    fn page_names_are_the_rail_labels() {
        let pages = page_names();
        assert_eq!(pages.get("agents").map(String::as_str), Some("Agents"));
        assert_eq!(pages.len(), PAGES.len());
    }
}
