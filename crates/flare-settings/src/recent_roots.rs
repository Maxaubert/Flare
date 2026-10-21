//! Recent and pinned folders, their labels and the + menu list (Task 4).
//!
//! Keys `flare.recentRoots` (history, capped at 12, the front is the newest) and `flare.pinnedRoots`
//! (pins in the order they were pinned). A visit never touches the pins; a pin never falls off history.

use std::collections::HashMap;

use crate::kv::PrefStore;

/// Kept in storage.
pub const RECENT_CAP: usize = 12;
/// Shown under the pins in the + menu.
pub const RECENT_SHOWN: usize = 5;

const RECENT_KEY: &str = "flare.recentRoots";
const PIN_KEY: &str = "flare.pinnedRoots";

/// Case-insensitive equality, the rule every list here uses.
fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The list after seeing `path`: it goes to the front, once, capped. A blank path changes nothing.
pub fn with_root(list: &[String], path: &str) -> Vec<String> {
    let p = path.trim();
    if p.is_empty() {
        return list.to_vec();
    }
    let mut out = vec![p.to_string()];
    out.extend(list.iter().filter(|x| !same(x, p)).cloned());
    out.truncate(RECENT_CAP);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub path: String,
    pub label: String,
}

/// The path split on `\` and `/`, empty runs dropped.
fn parts(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).filter(|s| !s.is_empty()).collect()
}

/// The last name of a path; a drive root reads as its drive letter ("D:").
fn base_name(path: &str) -> String {
    parts(path)
        .last()
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

/// The folder above the last name, or "" when there is none.
fn parent_name(path: &str) -> String {
    let p = parts(path);
    if p.len() >= 2 {
        p[p.len() - 2].to_string()
    } else {
        String::new()
    }
}

/// What to call each folder. A repeated name (case-insensitive) takes its parent along.
pub fn recent_labels(paths: &[String]) -> Vec<Label> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in paths {
        *counts.entry(base_name(p).to_lowercase()).or_insert(0) += 1;
    }
    paths
        .iter()
        .map(|p| {
            let name = base_name(p);
            let clash = counts.get(&name.to_lowercase()).copied().unwrap_or(0) >= 2;
            let parent = parent_name(p);
            let label = if clash && !parent.is_empty() {
                format!("{name}  \u{b7}  {parent}")
            } else {
                name
            };
            Label {
                path: p.clone(),
                label,
            }
        })
        .collect()
}

/// A stored JSON array of strings. Anything else (an object, junk) reads as empty; non-string and
/// empty entries are dropped.
fn read_list(s: &PrefStore, key: &str) -> Vec<String> {
    let Some(raw) = s.get(key) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(x) if !x.is_empty() => Some(x),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn write_list(s: &PrefStore, key: &str, list: &[String]) {
    if let Ok(text) = serde_json::to_string(list) {
        s.set(key, &text);
    }
}

pub fn recent_roots(s: &PrefStore) -> Vec<String> {
    read_list(s, RECENT_KEY)
}

pub fn remember_root(s: &PrefStore, path: &str) {
    let next = with_root(&recent_roots(s), path);
    write_list(s, RECENT_KEY, &next);
}

pub fn forget_root(s: &PrefStore, path: &str) {
    let next: Vec<String> = recent_roots(s)
        .into_iter()
        .filter(|x| !same(x, path))
        .collect();
    write_list(s, RECENT_KEY, &next);
}

pub fn pinned_roots(s: &PrefStore) -> Vec<String> {
    read_list(s, PIN_KEY)
}

pub fn is_pinned(s: &PrefStore, path: &str) -> bool {
    pinned_roots(s).iter().any(|p| same(p, path))
}

/// The pin list after toggling `path`: a pinned folder is removed, an unpinned one is appended, so
/// the menu keeps the pin order. A blank path changes nothing.
pub fn with_pin_toggled(pinned: &[String], path: &str) -> Vec<String> {
    let p = path.trim();
    if p.is_empty() {
        return pinned.to_vec();
    }
    if pinned.iter().any(|x| same(x, p)) {
        pinned.iter().filter(|x| !same(x, p)).cloned().collect()
    } else {
        let mut out = pinned.to_vec();
        out.push(p.to_string());
        out
    }
}

pub fn toggle_pin(s: &PrefStore, path: &str) {
    let next = with_pin_toggled(&pinned_roots(s), path);
    write_list(s, PIN_KEY, &next);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    pub pinned: bool,
}

/// The + menu: the pins in pin order, then the newest recents that are not pinned, `shown` of them.
pub fn plus_menu_list(pinned: &[String], recent: &[String], shown: usize) -> Vec<Place> {
    let rest = recent
        .iter()
        .filter(|r| !pinned.iter().any(|p| same(p, r)))
        .take(shown);
    pinned
        .iter()
        .map(|p| Place {
            path: p.clone(),
            pinned: true,
        })
        .chain(rest.map(|r| Place {
            path: r.clone(),
            pinned: false,
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    fn place(path: &str, pinned: bool) -> Place {
        Place {
            path: path.to_string(),
            pinned,
        }
    }

    #[test]
    fn with_root_puts_the_newest_first() {
        assert_eq!(with_root(&v(&["C:\\b"]), "C:\\a"), v(&["C:\\a", "C:\\b"]));
    }

    #[test]
    fn with_root_dedupes_case_insensitively() {
        assert_eq!(
            with_root(&v(&["C:\\a", "C:\\b", "C:\\c"]), "C:\\c"),
            v(&["C:\\c", "C:\\a", "C:\\b"])
        );
        assert_eq!(
            with_root(&v(&["C:\\Photos"]), "c:\\photos"),
            v(&["c:\\photos"])
        );
    }

    #[test]
    fn with_root_caps_at_twelve_and_ignores_blank() {
        let mut list = Vec::new();
        for i in 0..20 {
            list = with_root(&list, &format!("C:\\f{i}"));
        }
        assert_eq!(list.len(), 12);
        assert_eq!(list[0], "C:\\f19");
        assert_eq!(with_root(&v(&["C:\\x"]), "   "), v(&["C:\\x"]));
    }

    #[test]
    fn labels_use_the_name_alone() {
        let labels = recent_labels(&v(&["C:\\work\\atlas", "C:\\pics"]));
        assert_eq!(labels[0].label, "atlas");
        assert_eq!(labels[1].label, "pics");
    }

    #[test]
    fn labels_carry_the_parent_on_a_clash() {
        let labels = recent_labels(&v(&["C:\\site\\src", "C:\\app\\src"]));
        assert_eq!(labels[0].label, "src  \u{b7}  site");
        assert_eq!(labels[1].label, "src  \u{b7}  app");
    }

    #[test]
    fn labels_survive_trailing_separators_and_drive_roots() {
        assert_eq!(recent_labels(&v(&["C:\\shots\\"]))[0].label, "shots");
        assert_eq!(recent_labels(&v(&["D:\\"]))[0].label, "D:");
    }

    #[test]
    fn the_stored_list_round_trips_and_forget_removes_one() {
        let s = PrefStore::memory();
        remember_root(&s, "C:\\a");
        remember_root(&s, "C:\\b");
        assert_eq!(recent_roots(&s), v(&["C:\\b", "C:\\a"]));
        forget_root(&s, "c:\\a");
        assert_eq!(recent_roots(&s), v(&["C:\\b"]));
    }

    #[test]
    fn nonsense_in_storage_is_no_history() {
        let s = PrefStore::memory();
        s.set(RECENT_KEY, "{\"not\":\"an array\"}");
        assert!(recent_roots(&s).is_empty());
        s.set(RECENT_KEY, "soup");
        assert!(recent_roots(&s).is_empty());
    }

    #[test]
    fn pin_toggle() {
        assert_eq!(with_pin_toggled(&[], "C:\\a"), v(&["C:\\a"]));
        assert_eq!(
            with_pin_toggled(&v(&["C:\\a"]), "C:\\b"),
            v(&["C:\\a", "C:\\b"])
        );
        assert_eq!(
            with_pin_toggled(&v(&["C:\\a", "C:\\b"]), "c:\\A"),
            v(&["C:\\b"])
        );
        assert_eq!(with_pin_toggled(&v(&["C:\\a"]), "  "), v(&["C:\\a"]));
    }

    #[test]
    fn the_plus_menu_is_pins_then_recents() {
        let out = plus_menu_list(
            &v(&["C:\\old", "C:\\pinned"]),
            &v(&["C:\\pinned", "C:\\new", "C:\\old", "C:\\x"]),
            2,
        );
        assert_eq!(
            out,
            vec![
                place("C:\\old", true),
                place("C:\\pinned", true),
                place("C:\\new", false),
                place("C:\\x", false),
            ]
        );
    }

    #[test]
    fn a_pin_outlives_history() {
        let s = PrefStore::memory();
        toggle_pin(&s, "C:\\keep");
        forget_root(&s, "C:\\keep");
        for i in 0..20 {
            remember_root(&s, &format!("C:\\f{i}"));
        }
        assert_eq!(pinned_roots(&s), v(&["C:\\keep"]));
        assert!(is_pinned(&s, "c:\\KEEP"));
        let menu = plus_menu_list(&pinned_roots(&s), &recent_roots(&s), RECENT_SHOWN);
        assert_eq!(menu[0], place("C:\\keep", true));
        toggle_pin(&s, "C:\\keep");
        assert!(pinned_roots(&s).is_empty());
    }
}
