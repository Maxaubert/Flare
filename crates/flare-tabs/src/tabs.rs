//! Tab, TabState and every pure tab operation (Task 5).
//!
//! Sources: PT/core/shared/folderNames.ts, PT/src/renderer/src/lib/tabs.ts.

use crate::folder_names::{base_name, parent_name, path_parts};
use std::collections::{HashMap, HashSet};

/// What a tab is: a shell, or the Settings page riding the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Shell,
    Settings,
}

/// A placeholder drawn from the saved list before the restore settled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    Agent,
    Shell,
}

/// What a remote tab's host is: its logo on the tab (remote sessions PR 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Pc,
}

impl Os {
    /// `mac` or `pc`; any other word is no kind (rule 40: an unknown value reads as the default and is never
    /// written back).
    pub fn parse(s: &str) -> Option<Os> {
        match s {
            "mac" => Some(Os::Mac),
            "pc" => Some(Os::Pc),
            _ => None,
        }
    }
}

/// The machine a remote tab runs on (remote sessions PR 3): its id in `machines.json`, its name and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineTag {
    pub id: String,
    pub name: String,
    pub kind: Option<Os>,
}

impl MachineTag {
    /// A machine known only by its id (a restored tab before the machines are read): its name is the id.
    pub fn of_id(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            kind: None,
        }
    }
}

/// One shell and the folder it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub kind: TabKind,
    pub cwd: String,
    pub pending: Option<Pending>,
    /// Some for a remote tab (remote sessions PR 3, rule 287): a `TabKind::Shell` whose session runs on that
    /// machine. `cwd` is then a folder of that machine, never statted or opened here.
    pub remote: Option<MachineTag>,
}

/// The window's tab list and the id of the tab in front.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabState {
    pub tabs: Vec<Tab>,
    pub active_id: Option<String>,
}

pub const SETTINGS_ID: &str = "settings";

/// Appends and activates. Two tabs in one folder are allowed: a tab is a shell, not a root.
pub fn add_tab(s: &TabState, id: &str, cwd: &str) -> TabState {
    let mut tabs = s.tabs.clone();
    tabs.push(Tab {
        id: id.to_string(),
        kind: TabKind::Shell,
        cwd: cwd.to_string(),
        pending: None,
        remote: None,
    });
    TabState {
        tabs,
        active_id: Some(id.to_string()),
    }
}

/// `add_tab` for a remote tab: a shell tab on `machine`, in a folder of that machine (rule 287).
pub fn add_remote_tab(s: &TabState, id: &str, cwd: &str, machine: MachineTag) -> TabState {
    let mut out = add_tab(s, id, cwd);
    if let Some(t) = out.tabs.last_mut() {
        t.remote = Some(machine);
    }
    out
}

/// Closing the tab in front hands over to its right neighbour, else its left. Unknown id: unchanged.
pub fn close_tab(s: &TabState, id: &str) -> TabState {
    let Some(at) = s.tabs.iter().position(|t| t.id == id) else {
        return s.clone();
    };
    let mut tabs = s.tabs.clone();
    tabs.remove(at);
    if s.active_id.as_deref() != Some(id) {
        return TabState {
            tabs,
            active_id: s.active_id.clone(),
        };
    }
    // After the removal, `at` is the right neighbour and `at - 1` the left one.
    let next = tabs
        .get(at)
        .or_else(|| at.checked_sub(1).and_then(|i| tabs.get(i)));
    let active_id = next.map(|t| t.id.clone());
    TabState { tabs, active_id }
}

/// Fronts a tab. Unknown or already in front: unchanged.
pub fn pick_tab(s: &TabState, id: &str) -> TabState {
    let known = s.tabs.iter().any(|t| t.id == id);
    if !known || s.active_id.as_deref() == Some(id) {
        return s.clone();
    }
    TabState {
        tabs: s.tabs.clone(),
        active_id: Some(id.to_string()),
    }
}

/// Ctrl+Tab (dir 1) and its reverse (dir -1). Wraps. Fewer than two tabs: unchanged.
pub fn step_tab(s: &TabState, dir: i32) -> TabState {
    let len = s.tabs.len();
    if len < 2 {
        return s.clone();
    }
    let at = s
        .tabs
        .iter()
        .position(|t| Some(t.id.as_str()) == s.active_id.as_deref())
        .map_or(-1, |i| i as i64);
    let next = (at + i64::from(dir)).rem_euclid(len as i64) as usize;
    TabState {
        tabs: s.tabs.clone(),
        active_id: Some(s.tabs[next].id.clone()),
    }
}

/// None when the tab is unknown or the folder is the same ignoring case.
pub fn set_cwd(s: &TabState, id: &str, cwd: &str) -> Option<TabState> {
    let tab = s.tabs.iter().find(|t| t.id == id)?;
    if tab.cwd.to_lowercase() == cwd.to_lowercase() {
        return None;
    }
    let tabs = s
        .tabs
        .iter()
        .map(|t| {
            if t.id == id {
                Tab {
                    cwd: cwd.to_string(),
                    ..t.clone()
                }
            } else {
                t.clone()
            }
        })
        .collect();
    Some(TabState {
        tabs,
        active_id: s.active_id.clone(),
    })
}

/// One Settings tab, reused: asking again fronts it.
pub fn open_settings(s: &TabState) -> TabState {
    if s.tabs.iter().any(|t| t.kind == TabKind::Settings) {
        return TabState {
            tabs: s.tabs.clone(),
            active_id: Some(SETTINGS_ID.to_string()),
        };
    }
    let mut tabs = s.tabs.clone();
    tabs.push(Tab {
        id: SETTINGS_ID.to_string(),
        kind: TabKind::Settings,
        cwd: String::new(),
        pending: None,
        remote: None,
    });
    TabState {
        tabs,
        active_id: Some(SETTINGS_ID.to_string()),
    }
}

/// The cog is a toggle: with Settings in front it closes it and fronts `back` (when it is a real
/// tab), else the close neighbour. Otherwise it opens or fronts Settings.
pub fn toggle_settings(s: &TabState, back: Option<&str>) -> TabState {
    if s.active_id.as_deref() != Some(SETTINGS_ID) {
        return open_settings(s);
    }
    let closed = close_tab(s, SETTINGS_ID);
    match back {
        Some(b) if b != SETTINGS_ID => pick_tab(&closed, b),
        _ => closed,
    }
}

/// The tabs that own a shell: every tab but Settings.
pub fn shell_tabs(s: &TabState) -> Vec<&Tab> {
    s.tabs
        .iter()
        .filter(|t| t.kind != TabKind::Settings)
        .collect()
}

/// Moves a tab. `to_index` is the slot in the CURRENT list to land in front of; the removed slot
/// is corrected for. Unknown id: a copy of the list.
pub fn reorder_tabs(tabs: &[Tab], id: &str, to_index: usize) -> Vec<Tab> {
    let mut next = tabs.to_vec();
    let Some(from) = next.iter().position(|t| t.id == id) else {
        return next;
    };
    let moved = next.remove(from);
    let aim = if to_index > from {
        to_index - 1
    } else {
        to_index
    };
    let at = aim.min(next.len());
    next.insert(at, moved);
    next
}

/// One folder however it was spelled: case, slash direction, trailing separator.
fn folder_key(p: &str) -> String {
    path_parts(p).join("\\").to_lowercase()
}

/// The labels shown on the strip. A base name shared by tabs over different folders becomes
/// "<base> - <parent>" when a parent exists; two tabs in the same folder keep the plain name.
pub fn tab_labels(tabs: &[Tab]) -> Vec<String> {
    let bases: Vec<String> = tabs
        .iter()
        .map(|t| {
            if t.kind == TabKind::Settings {
                "Settings".to_string()
            } else {
                base_name(&t.cwd)
            }
        })
        .collect();
    let mut folders_by_base: HashMap<&str, HashSet<String>> = HashMap::new();
    for (t, base) in tabs.iter().zip(&bases) {
        folders_by_base
            .entry(base.as_str())
            .or_default()
            .insert(folder_key(&t.cwd));
    }
    tabs.iter()
        .zip(&bases)
        .map(|(t, base)| {
            let clash = folders_by_base.get(base.as_str()).map_or(0, HashSet::len) > 1;
            let parent = parent_name(&t.cwd);
            if clash && !parent.is_empty() {
                format!("{base} - {parent}")
            } else {
                base.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> TabState {
        let s = add_tab(&TabState::default(), "a", r"C:\w\api");
        let s = add_tab(&s, "b", r"C:\w\web");
        add_tab(&s, "c", r"D:\x\api")
    }

    fn ids(tabs: &[Tab]) -> Vec<&str> {
        tabs.iter().map(|t| t.id.as_str()).collect()
    }

    fn active(s: &TabState) -> Option<&str> {
        s.active_id.as_deref()
    }

    #[test]
    fn adds_at_the_end_and_activates() {
        let s = three();
        assert_eq!(ids(&s.tabs), ["a", "b", "c"]);
        assert_eq!(active(&s), Some("c"));
    }

    #[test]
    fn allows_two_tabs_in_the_same_folder() {
        let s = add_tab(&add_tab(&TabState::default(), "a", r"C:\w"), "b", r"C:\w");
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(tab_labels(&s.tabs), vec!["w", "w"]);
    }

    #[test]
    fn closing_the_active_tab_hands_over_right_then_left() {
        let s = three();
        assert_eq!(active(&close_tab(&pick_tab(&s, "b"), "b")), Some("c"));
        assert_eq!(active(&close_tab(&s, "c")), Some("b"));
    }

    #[test]
    fn closing_a_background_tab_keeps_the_active_one() {
        assert_eq!(active(&close_tab(&three(), "a")), Some("c"));
    }

    #[test]
    fn closing_the_last_tab_leaves_nothing_active() {
        let one = add_tab(&TabState::default(), "a", r"C:\w");
        assert_eq!(
            close_tab(&one, "a"),
            TabState {
                tabs: vec![],
                active_id: None
            }
        );
    }

    #[test]
    fn steps_with_wrap() {
        let s = three();
        assert_eq!(active(&step_tab(&s, 1)), Some("a"));
        assert_eq!(active(&step_tab(&pick_tab(&s, "a"), -1)), Some("c"));
    }

    #[test]
    fn set_cwd_relabels_and_reports_no_change() {
        let s = three();
        assert_eq!(set_cwd(&s, "a", r"C:\w\api"), None);
        assert_eq!(set_cwd(&s, "a", r"c:\W\API"), None);
        let moved = set_cwd(&s, "b", r"C:\w\web\src").expect("changed");
        assert_eq!(tab_labels(&moved.tabs)[1], "src");
    }

    #[test]
    fn same_names_are_told_apart_by_the_parent() {
        assert_eq!(tab_labels(&three().tabs), vec!["api - w", "web", "api - x"]);
    }

    #[test]
    fn one_folder_spelled_two_ways_is_not_a_clash() {
        let s = add_tab(
            &add_tab(&TabState::default(), "a", r"C:\w\api"),
            "b",
            "c:/w/api/",
        );
        assert_eq!(tab_labels(&s.tabs), vec!["api", "api"]);
    }

    #[test]
    fn a_drive_root_is_labelled_by_its_letter() {
        let s = add_tab(&TabState::default(), "a", r"D:\");
        assert_eq!(tab_labels(&s.tabs), vec!["D:"]);
    }

    #[test]
    fn settings_opens_once_and_is_reused() {
        let s = open_settings(&open_settings(&three()));
        assert_eq!(
            s.tabs
                .iter()
                .filter(|t| t.kind == TabKind::Settings)
                .count(),
            1
        );
        assert_eq!(
            tab_labels(&s.tabs).last().map(String::as_str),
            Some("Settings")
        );
        assert_eq!(shell_tabs(&s).len(), 3);
    }

    #[test]
    fn the_cog_toggles_back_to_the_tab_used_before() {
        let open = toggle_settings(&pick_tab(&three(), "a"), None);
        assert_eq!(active(&open), Some(SETTINGS_ID));
        let back = toggle_settings(&open, Some("a"));
        assert!(!back.tabs.iter().any(|t| t.kind == TabKind::Settings));
        assert_eq!(active(&back), Some("a"));
        assert_eq!(active(&toggle_settings(&open, None)), Some("c"));
        assert_eq!(
            active(&toggle_settings(&pick_tab(&open, "b"), None)),
            Some(SETTINGS_ID)
        );
    }

    #[test]
    fn reorders_correcting_for_the_removed_slot() {
        let s = three();
        assert_eq!(ids(&reorder_tabs(&s.tabs, "a", 3)), ["b", "c", "a"]);
        assert_eq!(ids(&reorder_tabs(&s.tabs, "c", 0)), ["c", "a", "b"]);
    }

    #[test]
    fn unknown_ids_change_nothing() {
        let s = three();
        assert_eq!(close_tab(&s, "zzz"), s);
        assert_eq!(pick_tab(&s, "zzz"), s);
        assert_eq!(ids(&reorder_tabs(&s.tabs, "zzz", 1)), ["a", "b", "c"]);
        let one = add_tab(&TabState::default(), "a", r"C:\w");
        assert_eq!(step_tab(&one, 1), one);
    }

    #[test]
    fn a_placeholder_keeps_its_pending_kind() {
        let mut s = three();
        s.tabs[0].pending = Some(Pending::Agent);
        let fronted = pick_tab(&s, "a");
        assert_eq!(fronted.tabs[0].pending, Some(Pending::Agent));
        let moved = reorder_tabs(&fronted.tabs, "a", 2);
        assert_eq!(moved[1].pending, Some(Pending::Agent));
        let renamed = set_cwd(&fronted, "a", r"E:\other").expect("changed");
        assert_eq!(renamed.tabs[0].pending, Some(Pending::Agent));
    }
}
