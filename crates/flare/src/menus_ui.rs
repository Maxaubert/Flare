//! places and tab menus, the +N list rows, TipHost (Task 26).
//!
//! Sources: `tabMenus.tsx`, `TabOverflowList.tsx`, `Tooltips.tsx`. Pure data; the controller
//! (Task 30) turns the rows into Slint structs and shows the panels.
#![allow(dead_code)] // the controller (Task 30) is the caller

use crate::tabs_ui::View;
use flare_settings::kv::PrefStore;
use flare_settings::recent_roots::{pinned_roots, recent_roots, toggle_pin};
use flare_tabs::places::place_rows;
use flare_tabs::sidebar_rows::find_keeps;
use flare_tabs::tip_place::{BoxF, TIP_SHOW_MS, TipSide, TipSpot, place_tip};

/// The glyph names a menu row may name, and the path each is drawn with (`ui/glyphs.slint`).
pub const GLYPH_FOLDER_FILL: &str = "folder-fill";
pub const GLYPH_PIN: &str = "pin";
pub const GLYPH_COPY: &str = "menu-copy";
pub const GLYPH_CLOSE: &str = "menu-close";
/// The terminal menu's glyphs (Milestone 5, PT `MenuIcon.tsx`).
pub const GLYPH_OPEN: &str = "menu-open";
pub const GLYPH_PASTE: &str = "menu-paste";
pub const GLYPH_LINK: &str = "menu-link";
pub const GLYPH_FOLDER: &str = "menu-folder";
pub const GLYPH_FIND: &str = "menu-find";
pub const GLYPH_HELP: &str = "menu-help";
/// The Surfaces rows: a side pane, a diff page and a checklist.
pub const GLYPH_PANEL: &str = "menu-panel";
pub const GLYPH_CHANGES: &str = "menu-changes";
pub const GLYPH_TASKS: &str = "menu-tasks";
/// The tab badges (plugins phase 1), one per `flare_ctl::decor::BADGE_ICONS` name.
pub const GLYPH_BADGE_PHONE: &str = "badge-phone";
pub const GLYPH_BADGE_GLOBE: &str = "badge-globe";
pub const GLYPH_BADGE_BELL: &str = "badge-bell";
pub const GLYPH_BADGE_MIC: &str = "badge-mic";
pub const GLYPH_BADGE_EYE: &str = "badge-eye";
pub const GLYPH_BADGE_MOON: &str = "badge-moon";
pub const GLYPH_BADGE_LOCK: &str = "badge-lock";
pub const GLYPH_BADGE_BOLT: &str = "badge-bolt";
pub const GLYPH_BADGE_STAR: &str = "badge-star";
pub const GLYPH_BADGE_PLUS: &str = "badge-plus";
pub const GLYPH_BADGE_PIN: &str = "badge-pin";

/// The glyph a badge icon is drawn with, for a built-in badge name (`flare_ctl::decor::BADGE_ICONS`).
pub fn badge_glyph(icon: &str) -> Option<&'static str> {
    Some(match icon {
        "phone" => GLYPH_BADGE_PHONE,
        "globe" => GLYPH_BADGE_GLOBE,
        "bell" => GLYPH_BADGE_BELL,
        "mic" => GLYPH_BADGE_MIC,
        "eye" => GLYPH_BADGE_EYE,
        "moon" => GLYPH_BADGE_MOON,
        "lock" => GLYPH_BADGE_LOCK,
        "bolt" => GLYPH_BADGE_BOLT,
        "star" => GLYPH_BADGE_STAR,
        "plus" => GLYPH_BADGE_PLUS,
        "pin" => GLYPH_BADGE_PIN,
        _ => return None,
    })
}

/// The SVG path of a glyph name (24-unit viewbox), as `ui/glyphs.slint` holds it.
pub fn glyph_path(name: &str) -> &'static str {
    match name {
        GLYPH_FOLDER_FILL => "M2.5 5.5h6.2l2 2.6h10.8v10.4H2.5z",
        GLYPH_PIN => "M15 3l6 6-3 1-4 4 .5 4.5L10 14l-6 6-1-1 6-6-4.5-4.5L9 8l4-4z",
        GLYPH_COPY => "M8 8h12v12H8zM16 8V4H4v12h4",
        GLYPH_CLOSE => "M7 7l10 10M17 7L7 17",
        GLYPH_OPEN => "M14 4h6v6M20 4l-9 9M18 13v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h6",
        GLYPH_PASTE => "M9 3.5h6v3H9zM7 5H4.5v15.5h15V5H17",
        GLYPH_LINK => {
            "M9 15l6-6M7.5 10.5l-2 2a3.5 3.5 0 0 0 5 5l2-2M16.5 13.5l2-2a3.5 3.5 0 0 0-5-5l-2 2"
        }
        GLYPH_FOLDER => "M2.5 5.5h6.2l2 2.6h10.8v10.4H2.5z",
        GLYPH_FIND => "M10.5 4.5a6 6 0 1 0 0 12 6 6 0 0 0 0-12zM15 15l5 5",
        GLYPH_HELP => {
            "M12 3.8a8.2 8.2 0 1 0 0 16.4 8.2 8.2 0 0 0 0-16.4zM9.6 9.4a2.5 2.5 0 1 1 3.7 2.2c-.8.5-1.3 1-1.3 1.9M12 16.6v.01"
        }
        GLYPH_PANEL => {
            "M4.5 5h15a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1h-15a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM14.5 5v14"
        }
        GLYPH_CHANGES => {
            "M6 3.5h12a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 19V5A1.5 1.5 0 0 1 6 3.5zM12 7.5v5M9.5 10h5M9.5 16h5"
        }
        GLYPH_TASKS => {
            "M4 6.5l1.7 1.7L9 4.8M12 7h8M4 12.5l1.7 1.7L9 11M12 13h8M4 18.5l1.7 1.7L9 17M12 19h8"
        }
        GLYPH_BADGE_PHONE => {
            "M5.5 3.5h3.5l1.5 4-2.2 1.6a11 11 0 0 0 6.6 6.6l1.6-2.2 4 1.5v3.5a2 2 0 0 1-2 2A16.5 16.5 0 0 1 3.5 5.5a2 2 0 0 1 2-2z"
        }
        GLYPH_BADGE_GLOBE => {
            "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM3.5 12h17M12 3.5a3.8 8.5 0 0 1 0 17a3.8 8.5 0 0 1 0-17z"
        }
        GLYPH_BADGE_BELL => "M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15zM10 20.5h4",
        GLYPH_BADGE_MIC => {
            "M9 6a3 3 0 0 1 6 0v5a3 3 0 0 1-6 0zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5v3"
        }
        GLYPH_BADGE_EYE => {
            "M2.5 12C5 7.7 8.2 5.5 12 5.5C15.8 5.5 19 7.7 21.5 12C19 16.3 15.8 18.5 12 18.5C8.2 18.5 5 16.3 2.5 12zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z"
        }
        GLYPH_BADGE_MOON => "M20 14.5A8.5 8.5 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z",
        GLYPH_BADGE_LOCK => "M5.5 11h13v9.5h-13zM8 11V8a4 4 0 0 1 8 0v3",
        GLYPH_BADGE_BOLT => "M13 2.5L5 13.5h6l-1 8 8-11h-6z",
        GLYPH_BADGE_STAR => {
            "M12 3l2.7 5.6 6.1.8-4.5 4.2 1.1 6.1L12 16.8l-5.4 2.9 1.1-6.1-4.5-4.2 6.1-.8z"
        }
        GLYPH_BADGE_PLUS => "M12 5v14M5 12h14",
        GLYPH_BADGE_PIN => "M15 3l6 6-3 1-4 4 .5 4.5L10 14l-6 6-1-1 6-6-4.5-4.5L9 8l4-4z",
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuRowData {
    pub label: String,
    pub hint: String,
    /// A glyph name (see [`glyph_path`]); "" for none.
    pub icon: &'static str,
    pub filled_icon: bool,
    pub disabled: bool,
    pub trailing_icon: Option<&'static str>,
    pub trailing_filled: bool,
    pub trailing_tip: String,
    /// Starts a new group: a wider gap and a rule above the row (the Surfaces rows of the terminal menu).
    pub group: bool,
    /// A row with a flyout of children (one level): the rows whose `parent` is its index (a plugin's group in the
    /// tab menu, plugins phase 4). It only opens the flyout.
    pub flyout: bool,
    /// The index of the flyout row this row hangs from; -1 for a row of the menu itself.
    pub parent: i32,
    /// A glyph path drawn instead of `icon` when not empty (a plugin's own icon, which is no named glyph).
    pub icon_cmd: String,
}

impl MenuRowData {
    fn plain(label: &str) -> Self {
        Self {
            label: label.to_string(),
            hint: String::new(),
            icon: "",
            filled_icon: false,
            disabled: false,
            trailing_icon: None,
            trailing_filled: false,
            trailing_tip: String::new(),
            group: false,
            flyout: false,
            parent: -1,
            icon_cmd: String::new(),
        }
    }

    /// A row with these words and nothing else (the plugins' rows build on it).
    pub fn labelled(label: &str) -> Self {
        Self::plain(label)
    }
}

pub enum MenuKind {
    Plus,
    Tab {
        id: String,
        cwd: String,
    },
    /// The terminal's menu; its rows and picks belong to `TermUi`.
    Term {
        tab: String,
    },
    /// The menu of a plugin's sidebar section (plugins phase 4): move it up or down, fold it.
    Section {
        sid: String,
    },
}

/// The + menu: pins first, then the newest unpinned recents. None at all: one disabled row.
/// Each row comes with the folder it opens ("" for the note).
pub fn plus_menu_rows(prefs: &PrefStore) -> Vec<(MenuRowData, String)> {
    let places = place_rows(&pinned_roots(prefs), &recent_roots(prefs));
    if places.is_empty() {
        let mut note = MenuRowData::plain("No recent folders");
        note.disabled = true;
        return vec![(note, String::new())];
    }
    places
        .into_iter()
        .map(|p| {
            let mut row = MenuRowData::plain(&p.label);
            row.icon = GLYPH_FOLDER_FILL;
            row.filled_icon = true;
            row.trailing_icon = Some(GLYPH_PIN);
            row.trailing_filled = p.pinned;
            row.trailing_tip = if p.pinned {
                "Unpin"
            } else {
                "Pin to this menu"
            }
            .to_string();
            (row, p.path)
        })
        .collect()
}

/// Toggles the pin of the row at `index`, then re-reads the list: the row climbs to the top while
/// the menu stands.
pub fn toggle_pin_in_menu(prefs: &PrefStore, rows: &mut Vec<(MenuRowData, String)>, index: usize) {
    let Some((_, path)) = rows.get(index) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    toggle_pin(prefs, &path.clone());
    *rows = plus_menu_rows(prefs);
}

/// A tab's own menu: Close tab (with its key) and, for a folder tab only, Copy folder path.
/// Settings has no folder, and copying "" would blank the clipboard.
pub fn tab_menu_rows(cwd: &str, close_hint: &str) -> Vec<MenuRowData> {
    let mut close = MenuRowData::plain("Close tab");
    close.icon = GLYPH_CLOSE;
    close.hint = close_hint.to_string();
    let mut rows = vec![close];
    if !cwd.is_empty() {
        let mut copy = MenuRowData::plain("Copy folder path");
        copy.icon = GLYPH_FOLDER;
        rows.push(copy);
    }
    rows
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverflowRowData {
    pub id: String,
    pub label: String,
    pub folder: String,
    pub needs: bool,
    pub loading: bool,
    pub settings: bool,
}

/// The +N list: the hidden tabs, or EVERY tab (Ctrl+Shift+K); tabs that need you first, each
/// group in strip order; `query` narrows by label or path.
pub fn overflow_rows(view: &View, all: bool, query: &str) -> Vec<OverflowRowData> {
    let listed = view
        .rows
        .iter()
        .filter(|r| all || r.mode == 2)
        .filter(|r| find_keeps(query, &r.label, &r.tip));
    let make = |r: &crate::tabs_ui::RowData| OverflowRowData {
        id: r.id.clone(),
        label: r.label.clone(),
        folder: r.folder.clone(),
        needs: r.need.is_some(),
        loading: r.loading,
        settings: r.settings,
    };
    let rows: Vec<OverflowRowData> = listed.map(make).collect();
    let (needy, rest): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.needs);
    needy.into_iter().chain(rest).collect()
}

struct Pending {
    text: String,
    item: BoxF,
    prefer: TipSide,
    due: u64,
}

/// The tooltip timer: the pointer must rest `TIP_SHOW_MS` on an item before its tip shows.
pub struct TipHost {
    pending: Option<Pending>,
    shown: bool,
}

impl Default for TipHost {
    fn default() -> Self {
        Self::new()
    }
}

impl TipHost {
    pub fn new() -> Self {
        Self {
            pending: None,
            shown: false,
        }
    }

    /// "" clears. The SAME text and box again does not restart the wait (nor hide a shown tip);
    /// a different one does.
    pub fn request(&mut self, text: &str, item: BoxF, prefer: TipSide, now: u64) {
        if text.is_empty() {
            self.clear();
            return;
        }
        if let Some(p) = &self.pending
            && p.text == text
            && p.item == item
        {
            return;
        }
        self.shown = false;
        self.pending = Some(Pending {
            text: text.to_string(),
            item,
            prefer,
            due: now + TIP_SHOW_MS,
        });
    }

    /// A press, a key, a wheel, focus lost.
    pub fn clear(&mut self) {
        self.pending = None;
        self.shown = false;
    }

    /// After `TIP_SHOW_MS`: the placed spot, once.
    pub fn poll(&mut self, now: u64, tip_size: (f64, f64), view: (f64, f64)) -> Option<TipSpot> {
        if self.shown {
            return None;
        }
        let p = self.pending.as_ref()?;
        if now < p.due {
            return None;
        }
        self.shown = true;
        Some(place_tip(p.item, tip_size, view, p.prefer))
    }

    /// When the next `poll` can answer, for the timer.
    pub fn next_due(&self) -> Option<u64> {
        if self.shown {
            return None;
        }
        self.pending.as_ref().map(|p| p.due)
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The text being waited on or shown.
    pub fn text(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs_ui::{MarkRole, ViewInput, build_view};
    use flare_settings::recent_roots::remember_root;
    use flare_tabs::model::TabModel;
    use std::collections::HashMap;

    fn labels(rows: &[(MenuRowData, String)]) -> Vec<&str> {
        rows.iter().map(|(r, _)| r.label.as_str()).collect()
    }

    #[test]
    fn the_tab_menu_offers_close_and_copy_path_for_a_folder_tab_only() {
        let rows = tab_menu_rows("C:\\a", "Ctrl+W");
        let names: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(names, ["Close tab", "Copy folder path"]);
        assert_eq!(rows[0].hint, "Ctrl+W");
        // PT's tab menu draws a folder on "Copy folder path" (tabMenus.tsx: icon folder), not a copy glyph.
        assert_eq!(rows[1].icon, GLYPH_FOLDER);
        let settings = tab_menu_rows("", "Ctrl+W");
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0].label, "Close tab");
    }

    #[test]
    fn the_plus_menu_is_pins_then_five_recents_or_a_disabled_note() {
        let prefs = PrefStore::memory();
        let empty = plus_menu_rows(&prefs);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].0.label, "No recent folders");
        assert!(empty[0].0.disabled);
        for i in (0..8).rev() {
            remember_root(&prefs, &format!(r"C:\r\recent{i}"));
        }
        toggle_pin(&prefs, r"C:\p\pinA");
        toggle_pin(&prefs, r"C:\p\pinB");
        let rows = plus_menu_rows(&prefs);
        assert_eq!(rows.len(), 7);
        assert!(rows[0].0.trailing_filled && rows[1].0.trailing_filled);
        assert!(rows[2..].iter().all(|(r, _)| !r.trailing_filled));
        assert_eq!(rows[0].1, r"C:\p\pinA");
        assert!(rows.iter().all(|(r, _)| !r.disabled));
    }

    #[test]
    fn pinning_in_the_menu_moves_the_row_up_and_keeps_the_menu() {
        let prefs = PrefStore::memory();
        for p in [r"C:\x\c", r"C:\x\b", r"C:\x\a"] {
            remember_root(&prefs, p);
        }
        let mut rows = plus_menu_rows(&prefs);
        assert_eq!(
            rows.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
            [r"C:\x\a", r"C:\x\b", r"C:\x\c"]
        );
        toggle_pin_in_menu(&prefs, &mut rows, 2);
        assert_eq!(
            rows.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
            [r"C:\x\c", r"C:\x\a", r"C:\x\b"]
        );
        assert_eq!(pinned_roots(&prefs), [r"C:\x\c"]);
        assert_eq!(rows[0].0.trailing_tip, "Unpin");
        assert_eq!(rows[1].0.trailing_tip, "Pin to this menu");
        assert_eq!(rows[2].0.trailing_tip, "Pin to this menu");
        // And back: unpinning drops it to the recents' place.
        toggle_pin_in_menu(&prefs, &mut rows, 0);
        assert!(pinned_roots(&prefs).is_empty());
    }

    #[test]
    fn glyph_paths_match_the_slint_file() {
        let slint = include_str!("../ui/glyphs.slint");
        for name in [
            GLYPH_FOLDER_FILL,
            GLYPH_PIN,
            GLYPH_COPY,
            GLYPH_CLOSE,
            GLYPH_OPEN,
            GLYPH_PASTE,
            GLYPH_LINK,
            GLYPH_FOLDER,
            GLYPH_FIND,
            GLYPH_HELP,
            GLYPH_PANEL,
            GLYPH_CHANGES,
            GLYPH_TASKS,
            GLYPH_BADGE_PHONE,
            GLYPH_BADGE_GLOBE,
            GLYPH_BADGE_BELL,
            GLYPH_BADGE_MIC,
            GLYPH_BADGE_EYE,
            GLYPH_BADGE_MOON,
            GLYPH_BADGE_LOCK,
            GLYPH_BADGE_BOLT,
            GLYPH_BADGE_STAR,
            GLYPH_BADGE_PLUS,
            GLYPH_BADGE_PIN,
        ] {
            let needle = format!("<string> {name}: \"{}\";", glyph_path(name));
            assert!(
                slint.contains(&needle),
                "glyph {name} differs from glyphs.slint"
            );
        }
    }

    #[test]
    fn every_built_in_badge_has_a_glyph_and_nothing_else_does() {
        for icon in flare_ctl::decor::BADGE_ICONS {
            let glyph = badge_glyph(icon).unwrap_or_else(|| panic!("{icon} has no glyph"));
            assert!(!glyph_path(glyph).is_empty(), "{icon}");
        }
        for icon in ["check", "dot", "", "close", "menu-copy"] {
            assert_eq!(badge_glyph(icon), None, "{icon:?}");
        }
    }

    fn forty_view(needs: &HashMap<String, MarkRole>) -> View {
        let mut m = TabModel::empty(0);
        for i in 0..40 {
            m.open_tab(&format!(r"C:\w\p{i}"));
        }
        let offsets = HashMap::new();
        let none = std::collections::HashSet::new();
        let black = slint::Color::from_rgb_u8(0, 0, 0);
        let marks = crate::mark_colours::UiMarks {
            working: black,
            finished: black,
            question: black,
            failed: black,
            rainbow: Vec::new(),
            ink: black,
        };
        let agent = crate::tabs_ui::AgentRows {
            sets: flare_agent::tab_look::TabSets {
                agents: &none,
                working: &none,
                question: &none,
                failed: &none,
                done: &none,
            },
            sw: flare_agent::tab_look::Switches {
                indicator: flare_agent::tab_mark::Indicator::Ring,
                done_on: true,
                question_on: true,
                failed_on: true,
                rainbow: true,
            },
            marks: &marks,
            kinds: HashMap::new(),
            failed_kinds: HashMap::new(),
            loading: std::collections::HashSet::new(),
        };
        build_view(&ViewInput {
            model: &m,
            home: r"C:\Users\Admin",
            query: "",
            strip_phys: 1000,
            scale: 1.0,
            needs,
            offsets: &offsets,
            carried: None,
            agent: &agent,
        })
    }

    #[test]
    fn the_overflow_rows_list_hidden_tabs_or_all_and_need_first() {
        let plain = forty_view(&HashMap::new());
        let hidden: Vec<&str> = plain
            .rows
            .iter()
            .filter(|r| r.mode == 2)
            .map(|r| r.id.as_str())
            .collect();
        let listed = overflow_rows(&plain, false, "");
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            hidden
        );
        assert_eq!(overflow_rows(&plain, true, "").len(), 40);

        // Two tabs need you: they come first, flagged.
        let a = plain.rows[5].id.clone();
        let b = plain.rows[3].id.clone();
        let needs = HashMap::from([(a.clone(), MarkRole::Done), (b.clone(), MarkRole::Failed)]);
        let with = forty_view(&needs);
        let all = overflow_rows(&with, true, "");
        assert_eq!(all.len(), 40);
        assert_eq!(all[0].id, b);
        assert_eq!(all[1].id, a);
        assert!(all[0].needs && all[1].needs && all[2..].iter().all(|r| !r.needs));
        // The query narrows by label or path.
        let found = overflow_rows(&with, true, "p17");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "p17");
        let by_path = overflow_rows(&with, true, r"c:\w\p3");
        assert!(by_path.len() >= 10);
    }

    fn bx(l: f64, t: f64, w: f64, h: f64) -> BoxF {
        BoxF {
            left: l,
            top: t,
            width: w,
            height: h,
        }
    }

    #[test]
    fn a_tip_shows_after_one_second_and_only_once() {
        let mut h = TipHost::new();
        h.request("New tab", bx(500.0, 4.0, 32.0, 32.0), TipSide::Below, 0);
        assert_eq!(h.next_due(), Some(1000));
        assert!(h.poll(999, (80.0, 26.0), (1200.0, 800.0)).is_none());
        assert!(h.poll(1000, (80.0, 26.0), (1200.0, 800.0)).is_some());
        assert!(h.poll(1100, (80.0, 26.0), (1200.0, 800.0)).is_none());
        assert!(h.shown());
        assert_eq!(h.next_due(), None);
    }

    #[test]
    fn the_same_request_does_not_restart_the_wait_a_new_one_does() {
        let item = bx(500.0, 4.0, 32.0, 32.0);
        let mut h = TipHost::new();
        h.request("New tab", item, TipSide::Below, 0);
        h.request("New tab", item, TipSide::Below, 600);
        assert!(h.poll(1000, (80.0, 26.0), (1200.0, 800.0)).is_some());

        let mut g = TipHost::new();
        g.request("New tab", item, TipSide::Below, 0);
        g.request("Settings", item, TipSide::Below, 600);
        assert!(g.poll(1000, (80.0, 26.0), (1200.0, 800.0)).is_none());
        assert!(g.poll(1600, (80.0, 26.0), (1200.0, 800.0)).is_some());
        // A different box with the same text restarts too.
        let mut k = TipHost::new();
        k.request("New tab", item, TipSide::Below, 0);
        k.request("New tab", bx(600.0, 4.0, 32.0, 32.0), TipSide::Below, 600);
        assert!(k.poll(1000, (80.0, 26.0), (1200.0, 800.0)).is_none());
    }

    #[test]
    fn a_clear_stops_a_pending_tip_and_hides_a_shown_one() {
        let item = bx(500.0, 4.0, 32.0, 32.0);
        let mut h = TipHost::new();
        h.request("New tab", item, TipSide::Below, 0);
        h.clear();
        assert!(h.poll(2000, (80.0, 26.0), (1200.0, 800.0)).is_none());
        h.request("New tab", item, TipSide::Below, 0);
        assert!(h.poll(1000, (80.0, 26.0), (1200.0, 800.0)).is_some());
        assert!(h.shown());
        h.clear();
        assert!(!h.shown());
        // An empty text is a clear.
        h.request("New tab", item, TipSide::Below, 0);
        h.request("", item, TipSide::Below, 10);
        assert_eq!(h.next_due(), None);
    }

    #[test]
    fn a_tip_is_centred_six_below_and_inside_the_edge() {
        let mut h = TipHost::new();
        h.request("New tab", bx(500.0, 4.0, 32.0, 32.0), TipSide::Below, 0);
        let s = h.poll(1000, (80.0, 26.0), (1200.0, 800.0)).unwrap();
        assert_eq!(s.x + 40.0, 516.0);
        assert_eq!(s.y, 42.0);
        let mut c = TipHost::new();
        c.request("Settings", bx(1130.0, 4.0, 32.0, 28.0), TipSide::Below, 0);
        let s = c.poll(1000, (80.0, 26.0), (1200.0, 800.0)).unwrap();
        assert!(s.x + 80.0 <= 1194.0);
    }

    #[test]
    fn a_sidebar_row_tip_sits_beside_the_row() {
        let mut h = TipHost::new();
        h.request("C:\\a", bx(0.0, 100.0, 220.0, 46.0), TipSide::Right, 0);
        let s = h.poll(1000, (120.0, 26.0), (1200.0, 800.0)).unwrap();
        assert_eq!(s.side, TipSide::Right);
        assert_eq!(s.x, 226.0);
    }

    #[test]
    fn labels_helper_reads_rows() {
        let prefs = PrefStore::memory();
        assert_eq!(labels(&plus_menu_rows(&prefs)), ["No recent folders"]);
    }
}
