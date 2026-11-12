//! The Settings pages, in the rail's order, and the page each section sits on (M7 Task 9).
//! Source: `settingsIndex.ts` (`SETTINGS_PAGES`, `PAGE_OF`). Prism Terminal puts the theme on
//! Appearance and the shell, text and command help on Terminal; Flare keeps that split.

/// One page of the rail. `end` pins the page to the bottom of the rail (About).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageDef {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub end: bool,
}

/// The eight pages, in the rail's order. Machines (remote sessions PR 7) follows Plugins.
pub const PAGES: [PageDef; 8] = [
    PageDef {
        id: "appearance",
        label: "Appearance",
        icon: "appearance",
        end: false,
    },
    PageDef {
        id: "terminal",
        label: "Terminal",
        icon: "terminal",
        end: false,
    },
    PageDef {
        id: "agents",
        label: "Agents",
        icon: "agents",
        end: false,
    },
    PageDef {
        id: "dictation",
        label: "Dictation",
        icon: "dictation",
        end: false,
    },
    PageDef {
        id: "mods",
        label: "Plugins",
        icon: "chip",
        end: false,
    },
    PageDef {
        id: "machines",
        label: "Machines",
        icon: "machines",
        end: false,
    },
    PageDef {
        id: "diagnostics",
        label: "Diagnostics",
        icon: "diagnostics",
        end: false,
    },
    PageDef {
        id: "about",
        label: "About",
        icon: "about",
        end: true,
    },
];

/// The page a Settings tab opens on.
pub const DEFAULT_PAGE: &str = "appearance";

/// The page that holds a core section here. `None` for an id that is no core section, and for
/// this app's own sections (`window`, `opening`, `keys`, `about`), which the page builders
/// place directly.
pub fn page_of_section(section_id: &str) -> Option<&'static str> {
    match section_id {
        "shell" | "text" | "help" => Some("terminal"),
        "theme" => Some("appearance"),
        "marks" | "claude" | "colours" => Some("agents"),
        "dictation" | "listening" | "while" | "models" | "gpu" | "anywhere" => Some("dictation"),
        "diagnostics" => Some("diagnostics"),
        _ => None,
    }
}

/// The label of a page, by id.
pub fn page_label(id: &str) -> Option<&'static str> {
    PAGES
        .iter()
        .find(|page| page.id == id)
        .map(|page| page.label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::is_setting_icon;
    use crate::sections::{APP_SECTIONS, SECTIONS};

    #[test]
    fn the_rail_order_is_the_reference_with_about_last_and_at_the_end() {
        let ids: Vec<&str> = PAGES.iter().map(|page| page.id).collect();
        assert_eq!(
            ids,
            [
                "appearance",
                "terminal",
                "agents",
                "dictation",
                "mods",
                "machines",
                "diagnostics",
                "about"
            ]
        );
        let ends: Vec<&str> = PAGES
            .iter()
            .filter(|page| page.end)
            .map(|page| page.id)
            .collect();
        assert_eq!(ends, ["about"]);
        assert!(PAGES.iter().any(|page| page.id == DEFAULT_PAGE));
        assert_eq!(page_label("agents"), Some("Agents"));
        assert_eq!(page_label("nope"), None);
    }

    #[test]
    fn every_core_section_belongs_to_a_page() {
        for (id, _) in SECTIONS {
            let page = page_of_section(id).unwrap_or_else(|| panic!("section {id} has no page"));
            assert!(
                PAGES.iter().any(|p| p.id == page),
                "section {id} names an unknown page {page}"
            );
        }
        assert_eq!(page_of_section("shell"), Some("terminal"));
        assert_eq!(page_of_section("theme"), Some("appearance"));
        assert_eq!(page_of_section("colours"), Some("agents"));
        assert_eq!(page_of_section("gpu"), Some("dictation"));
        assert_eq!(page_of_section("diagnostics"), Some("diagnostics"));
    }

    #[test]
    fn app_sections_are_not_core_sections_and_have_no_page_here() {
        for (id, _) in APP_SECTIONS {
            assert_eq!(
                page_of_section(id),
                None,
                "app section {id} must not map through the core table"
            );
        }
        assert_eq!(page_of_section("nope"), None);
    }

    #[test]
    fn every_page_icon_is_a_known_icon() {
        for page in PAGES.iter() {
            assert!(
                is_setting_icon(page.icon),
                "page {} has no icon {}",
                page.id,
                page.icon
            );
        }
    }
}
