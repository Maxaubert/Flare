//! The click target decision for a link (Prism Terminal's TerminalPanel `linkHandler.activate`,
//! `shownLinkOver` and `openTermPath`, with `termOsc8.ts osc8Target`). A plain left click opens
//! a link; any other button opens nothing, and the menu offers Open link instead. Only http and
//! https addresses reach `flare_win::shell_open::open_url`, and a path is handed over as text and
//! the folder it was found from; the backend re-resolves it and applies the allowlist.

use crate::link_scan::LinkTarget;
use crate::links::find_links;
use crate::osc8::{is_web_link, osc8_target};
use crate::path_open::PathKind;
use crate::web_url::is_web_url;

/// A file link under the pointer (D5, a path between marks since 2026-11-11): what the menu offers and a click
/// opens. kind is what the text says (a trailing separator is a folder); nothing is statted for the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermPathAt {
    pub kind: PathKind,
    pub abs: String,
    pub text: String,
    pub base: String,
    /// The line the link named, for the panel.
    pub line: Option<u32>,
    /// The link's other spellings (`file_marks::FileMark::alts`): every row that acts on the path takes, on its
    /// worker, the first of `text` and these that names something, as a plain click does.
    pub alts: Vec<String>,
}

impl TermPathAt {
    /// `text`, then `alts`: the spellings a worker tries in order.
    pub fn spellings(&self) -> Vec<String> {
        std::iter::once(self.text.clone())
            .chain(self.alts.iter().cloned())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    OpenUrl(String),
    OpenPath { base: String, text: String },
    Nothing,
}

/// A plain left click. Web: the url, when `is_web_url`. Osc8: `osc8_target(uri, shown)`; opens only when the
/// uri AND the target pass `is_web_link` (http/https). Path: open (the backend re-resolves and applies the
/// allowlist). Any other button: Nothing (right click opens nothing; the menu offers Open link).
pub fn click_action(
    t: &LinkTarget,
    shown: Option<&str>,
    base: Option<&str>,
    button: u8,
) -> LinkAction {
    if button != 0 {
        return LinkAction::Nothing;
    }
    match t {
        LinkTarget::Web(url) => {
            if is_web_url(url) {
                LinkAction::OpenUrl(url.clone())
            } else {
                LinkAction::Nothing
            }
        }
        LinkTarget::Osc8 { uri } => {
            if !is_web_link(uri) {
                return LinkAction::Nothing;
            }
            let target = osc8_target(uri, shown);
            if is_web_link(&target) {
                LinkAction::OpenUrl(target)
            } else {
                LinkAction::Nothing
            }
        }
        LinkTarget::Path { text, .. } => match base {
            Some(base) => LinkAction::OpenPath {
                base: base.to_string(),
                text: text.clone(),
            },
            None => LinkAction::Nothing,
        },
    }
}

/// D6: Prism Terminal opens every link on a plain click. One place to change it.
pub fn link_click_needs_ctrl(_t: &LinkTarget) -> bool {
    false
}

/// The first web link in `line_text` (char ranges from `find_links`) that overlaps chars `from..to`: the address a
/// label SHOWS (xterm `shownLinkOver`), as text.
pub fn shown_link_over(line_text: &str, from: usize, to: usize) -> Option<String> {
    let chars: Vec<char> = line_text.chars().collect();
    find_links(line_text)
        .into_iter()
        .find(|span| span.start < to && from < span.end)
        .map(|span| chars[span.start..span.end].iter().collect())
}

/// The text the menu copies and opens for a link: an OSC 8 label uses `osc8_target`, a web link its text.
pub fn menu_link(t: &LinkTarget, shown: Option<&str>) -> String {
    match t {
        LinkTarget::Web(text) => text.clone(),
        LinkTarget::Path { text, .. } => text.clone(),
        LinkTarget::Osc8 { uri } => osc8_target(uri, shown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(s: &str) -> LinkTarget {
        LinkTarget::Web(s.to_string())
    }

    fn osc8(s: &str) -> LinkTarget {
        LinkTarget::Osc8 { uri: s.to_string() }
    }

    #[test]
    fn a_left_click_on_a_web_link_opens_it() {
        assert_eq!(
            click_action(&web("https://a.io/x"), None, None, 0),
            LinkAction::OpenUrl("https://a.io/x".to_string())
        );
    }

    #[test]
    fn any_other_button_opens_nothing() {
        assert_eq!(
            click_action(&web("https://a.io/x"), None, None, 1),
            LinkAction::Nothing
        );
        assert_eq!(
            click_action(&web("https://a.io/x"), None, None, 2),
            LinkAction::Nothing
        );
    }

    #[test]
    fn a_non_http_scheme_never_opens() {
        for text in ["javascript:alert(1)", "file:///C:/x", "ftp://h/x"] {
            assert_eq!(
                click_action(&web(text), None, None, 0),
                LinkAction::Nothing,
                "{text}"
            );
        }
    }

    #[test]
    fn an_osc8_label_showing_another_host_opens_what_it_shows() {
        assert_eq!(
            click_action(
                &osc8("https://evil.io/"),
                Some("https://github.com/x"),
                None,
                0
            ),
            LinkAction::OpenUrl("https://github.com/x".to_string())
        );
    }

    #[test]
    fn an_osc8_label_with_no_shown_address_opens_its_uri() {
        assert_eq!(
            click_action(&osc8("https://evil.io/"), None, None, 0),
            LinkAction::OpenUrl("https://evil.io/".to_string())
        );
    }

    #[test]
    fn an_osc8_file_uri_opens_nothing() {
        assert_eq!(
            click_action(&osc8("file:///C:/x"), None, None, 0),
            LinkAction::Nothing
        );
    }

    #[test]
    fn a_path_click_hands_over_the_text_and_the_folder_it_was_found_from() {
        let t = LinkTarget::path("src\\a.rs");

        assert_eq!(
            click_action(&t, None, Some("C:\\r"), 0),
            LinkAction::OpenPath {
                base: "C:\\r".to_string(),
                text: "src\\a.rs".to_string(),
            }
        );
        assert_eq!(click_action(&t, None, None, 0), LinkAction::Nothing);
    }

    #[test]
    fn no_link_needs_ctrl_today() {
        assert!(!link_click_needs_ctrl(&web("https://a.io/x")));
        assert!(!link_click_needs_ctrl(&osc8("https://a.io/x")));
        assert!(!link_click_needs_ctrl(&LinkTarget::path("a.rs")));
    }

    #[test]
    fn shown_link_over_reads_the_address_in_the_label_cells() {
        let line = "PR https://x.io/p/1 done";
        assert_eq!(
            shown_link_over(line, 3, 20),
            Some("https://x.io/p/1".to_string())
        );
        assert_eq!(shown_link_over(line, 0, 2), None);
    }

    #[test]
    fn the_menu_link_is_what_a_click_would_open() {
        let t = osc8("https://evil.io/");
        assert_eq!(
            menu_link(&t, Some("https://github.com/x")),
            osc8_target("https://evil.io/", Some("https://github.com/x"))
        );
        assert_eq!(menu_link(&web("https://a.io/x"), None), "https://a.io/x");
    }
}
