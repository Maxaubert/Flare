//! places for the + menu and the start screen (Task 18).
//!
//! Sources: PT EmptyState.tsx `whereabouts`, tabMenus.tsx `plusMenuRows`.

use crate::folder_names::{base_name, path_parts};
use flare_settings::recent_roots::{RECENT_SHOWN, plus_menu_list, recent_labels};

/// One folder offered by the + menu or the start screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceRow {
    pub path: String,
    pub name: String,
    pub near: String,
    pub label: String,
    pub pinned: bool,
}

/// Where a folder lives: the last two folders above it, with a leading ellipsis when more exist.
pub fn whereabouts(p: &str) -> String {
    let parts = path_parts(p);
    let above = &parts[..parts.len().saturating_sub(1)];
    let near = above[above.len().saturating_sub(2)..].join("\\");
    if above.len() > 2 {
        format!("\u{2026}\\{near}")
    } else {
        near
    }
}

/// The pins, then the newest recents that are not pinned, each with the text the menu shows.
pub fn place_rows(pinned: &[String], recent: &[String]) -> Vec<PlaceRow> {
    let places = plus_menu_list(pinned, recent, RECENT_SHOWN);
    let paths: Vec<String> = places.iter().map(|p| p.path.clone()).collect();
    let labels = recent_labels(&paths);
    places
        .into_iter()
        .enumerate()
        .map(|(i, p)| PlaceRow {
            name: base_name(&p.path),
            near: whereabouts(&p.path),
            label: labels
                .get(i)
                .map_or_else(|| base_name(&p.path), |l| l.label.clone()),
            pinned: p.pinned,
            path: p.path,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn whereabouts_shows_the_last_two_folders_above() {
        assert_eq!(
            whereabouts(r"C:\Users\Admin\Documents\Claude\Github\Prism"),
            "\u{2026}\\Claude\\Github"
        );
        assert_eq!(whereabouts(r"C:\work\prism"), r"C:\work");
        assert_eq!(whereabouts(r"C:\a"), "C:");
        assert_eq!(whereabouts(r"D:\"), "");
    }

    #[test]
    fn place_rows_are_pins_then_recents_with_menu_labels() {
        let rows = place_rows(
            &v(&[r"C:\site\src"]),
            &v(&[r"C:\app\src", r"C:\site\src", r"C:\pics"]),
        );
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, [r"C:\site\src", r"C:\app\src", r"C:\pics"]);
        let pins: Vec<bool> = rows.iter().map(|r| r.pinned).collect();
        assert_eq!(pins, [true, false, false]);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["src  \u{b7}  site", "src  \u{b7}  app", "pics"]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["src", "src", "pics"]);
    }
}
