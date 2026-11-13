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

/// Remote tabs' folders (rule 1213), kept apart from this machine's: `flare.recentRemoteRoots`, a JSON array of
/// `{"machine": id, "path": folder}`, the front the newest, capped like the local list. An older Flare never reads it.
const REMOTE_KEY: &str = "flare.recentRemoteRoots";
/// The most remote folders the + menu shows, after this machine's.
pub const REMOTE_SHOWN: usize = 3;

/// A folder on another machine, by the machine's id in `machines.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRoot {
    pub machine: String,
    pub path: String,
}

/// The remote list after seeing `path` on `machine`: it goes to the front, once, capped. A blank path or machine
/// changes nothing. Two folders are the same by their exact path; `fold_case` (a Windows host's folders, rule 1215)
/// also takes them as one whatever their case, as that host does. A Mac or Linux host keeps `/x/App` and `/x/app` apart.
pub fn with_remote_root(
    list: &[RemoteRoot],
    machine: &str,
    path: &str,
    fold_case: bool,
) -> Vec<RemoteRoot> {
    let (m, p) = (machine.trim(), path.trim());
    if m.is_empty() || p.is_empty() {
        return list.to_vec();
    }
    let mut out = vec![RemoteRoot {
        machine: m.to_string(),
        path: p.to_string(),
    }];
    out.extend(
        list.iter()
            .filter(|x| !(x.machine == m && (x.path == p || (fold_case && same(&x.path, p)))))
            .cloned(),
    );
    out.truncate(RECENT_CAP);
    out
}

/// The stored remote folders; anything that is not an entry with a machine and a path is dropped.
pub fn remote_roots(s: &PrefStore) -> Vec<RemoteRoot> {
    let Some(raw) = s.get(REMOTE_KEY) else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&raw)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let machine = v.get("machine")?.as_str().filter(|m| !m.is_empty())?;
            let path = v.get("path")?.as_str().filter(|p| !p.is_empty())?;
            Some(RemoteRoot {
                machine: machine.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

/// [`with_remote_root`] on the stored list; `fold_case` for a Windows host (rule 1215).
pub fn remember_remote_root(s: &PrefStore, machine: &str, path: &str, fold_case: bool) {
    let next = with_remote_root(&remote_roots(s), machine, path, fold_case);
    let list: Vec<serde_json::Value> = next
        .iter()
        .map(|r| serde_json::json!({"machine": r.machine, "path": r.path}))
        .collect();
    if let Ok(text) = serde_json::to_string(&list) {
        s.set(REMOTE_KEY, &text);
    }
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

/// The starter folders (Documents, Downloads, Desktop, the user folder) that fill the + menu's empty spots: while
/// fewer than `shown` unpinned recents are listed, the first starters that are neither pinned nor in the history,
/// each once, make up the rest. A real recent pushes one out.
pub fn starter_fill(
    pinned: &[String],
    recent: &[String],
    starters: &[String],
    shown: usize,
) -> Vec<String> {
    let listed = recent
        .iter()
        .filter(|r| !pinned.iter().any(|p| same(p, r)))
        .take(shown)
        .count();
    let mut out: Vec<String> = Vec::new();
    for s in starters {
        if out.len() + listed >= shown {
            break;
        }
        let known = pinned.iter().chain(recent).chain(&out).any(|x| same(x, s));
        if !s.trim().is_empty() && !known {
            out.push(s.clone());
        }
    }
    out
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
    fn starters_fill_the_empty_spots_and_recents_push_them_out() {
        let starters = v(&[
            "C:\\u\\Documents",
            "C:\\u\\Downloads",
            "C:\\u\\Desktop",
            "C:\\u",
        ]);
        // Nothing yet: every starter, in order.
        assert_eq!(starter_fill(&[], &[], &starters, 5), starters);
        // Three recents leave two spots.
        let recent = v(&["C:\\a", "C:\\b", "C:\\c"]);
        assert_eq!(
            starter_fill(&[], &recent, &starters, 5),
            v(&["C:\\u\\Documents", "C:\\u\\Downloads"])
        );
        // A full menu has no spot left.
        let five = v(&["C:\\a", "C:\\b", "C:\\c", "C:\\d", "C:\\e"]);
        assert!(starter_fill(&[], &five, &starters, 5).is_empty());
    }

    #[test]
    fn a_starter_already_pinned_or_recent_is_never_listed_twice() {
        let starters = v(&[
            "C:\\u\\Documents",
            "C:\\u\\Downloads",
            "C:\\u\\documents",
            "C:\\u",
        ]);
        // A pin takes no recent's spot, but a pinned or recent starter is not offered again, nor one twice.
        let out = starter_fill(&v(&["C:\\U\\DOWNLOADS"]), &v(&["c:\\u"]), &starters, 5);
        assert_eq!(out, v(&["C:\\u\\Documents"]));
        let out = starter_fill(&[], &v(&["C:\\a", "C:\\u\\Documents"]), &starters, 5);
        assert_eq!(out, v(&["C:\\u\\Downloads", "C:\\u"]));
    }

    /// Rule 1213: a remote folder is kept with its machine, apart from this machine's list: newest first, once per
    /// machine and folder, capped; junk in storage reads as nothing.
    #[test]
    fn remote_folders_are_kept_with_their_machine_newest_first() {
        let s = PrefStore::memory();
        remember_root(&s, "C:\\local");
        remember_remote_root(&s, "mac", "/Users/max/proj", false);
        remember_remote_root(&s, "pc", "/Users/max/proj", true);
        remember_remote_root(&s, "mac", "/Users/max/notes", false);
        remember_remote_root(&s, "mac", "/Users/max/proj", false);
        let r = |m: &str, p: &str| RemoteRoot {
            machine: m.into(),
            path: p.into(),
        };
        assert_eq!(
            remote_roots(&s),
            vec![
                r("mac", "/Users/max/proj"),
                r("mac", "/Users/max/notes"),
                r("pc", "/Users/max/proj"),
            ]
        );
        assert_eq!(
            recent_roots(&s),
            v(&["C:\\local"]),
            "the local list is untouched"
        );
        remember_remote_root(&s, "", "/x", false);
        remember_remote_root(&s, "mac", "  ", false);
        assert_eq!(remote_roots(&s).len(), 3);
        for i in 0..20 {
            remember_remote_root(&s, "mac", &format!("/f{i}"), false);
        }
        assert_eq!(remote_roots(&s).len(), RECENT_CAP);
        s.set(
            REMOTE_KEY,
            "[\"/a\", {\"machine\": \"mac\"}, {\"machine\": \"m\", \"path\": \"/p\"}]",
        );
        assert_eq!(remote_roots(&s), vec![r("m", "/p")]);
        s.set(REMOTE_KEY, "soup");
        assert!(remote_roots(&s).is_empty());
    }

    /// Rule 1215: on a Mac or Linux host `/x/App` and `/x/app` are two folders and both are kept; a Windows host's folders
    /// are one whatever their case; the same path is always one.
    #[test]
    fn remote_folders_differ_by_case_except_on_a_windows_host() {
        let r = |m: &str, p: &str| RemoteRoot {
            machine: m.into(),
            path: p.into(),
        };
        let mac = with_remote_root(&[r("mac", "/x/App")], "mac", "/x/app", false);
        assert_eq!(mac, vec![r("mac", "/x/app"), r("mac", "/x/App")]);
        assert_eq!(
            with_remote_root(&mac, "mac", "/x/App", false),
            vec![r("mac", "/x/App"), r("mac", "/x/app")]
        );
        assert_eq!(
            with_remote_root(&[r("pc", "C:\\Work")], "pc", "c:\\work", true),
            vec![r("pc", "c:\\work")]
        );
        // Another machine's folder of the same path is never touched.
        assert_eq!(
            with_remote_root(&[r("pc", "C:\\Work")], "pc2", "c:\\work", true).len(),
            2
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
