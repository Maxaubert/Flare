//! places and tab menus, the +N list rows, TipHost (Task 26).
//!
//! Sources: `tabMenus.tsx`, `TabOverflowList.tsx`, `Tooltips.tsx`. Pure data; the controller
//! (Task 30) turns the rows into Slint structs and shows the panels.
#![allow(dead_code)] // the controller (Task 30) is the caller

use crate::tabs_ui::View;
use flare_settings::kv::PrefStore;
use flare_settings::recent_roots::{
    RECENT_SHOWN, REMOTE_SHOWN, RemoteRoot, pinned_roots, recent_labels, recent_roots,
    remote_roots, starter_fill, toggle_pin,
};
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
/// The Surfaces rows: a side pane and a diff page.
pub const GLYPH_PANEL: &str = "menu-panel";
pub const GLYPH_CHANGES: &str = "menu-changes";
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
/// A remote machine's system (remote sessions PR 7): filled logos on the 24-unit box.
pub const GLYPH_LOGO_MAC: &str = "logo-mac";
pub const GLYPH_LOGO_PC: &str = "logo-pc";

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
        GLYPH_BADGE_PHONE => {
            "M6 3.5h7a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 19V5A1.5 1.5 0 0 1 6 3.5zM8.5 17.5h2M17.5 9a3.5 3.5 0 0 1 0 6M20 6.5a7 7 0 0 1 0 11"
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
        GLYPH_LOGO_MAC => {
            "M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701"
        }
        GLYPH_LOGO_PC => {
            "M0 0h11.377v11.372H0zm12.623 0H24v11.372H12.623zM0 12.623h11.377V24H0zm12.623 0H24V24H12.623z"
        }
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
    /// A plugin's brand logo drawn in place of the icon (its item in the tab menu, 2026-11-08): the path, filled in the
    /// colour (`#rrggbb`) on a white rounded tile.
    pub logo: Option<(String, String)>,
}

impl MenuRowData {
    pub(crate) fn plain(label: &str) -> Self {
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
            logo: None,
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
    /// Ctrl+Shift+R (rule 1236): the linked machines alone, the + menu's machine rows; a pick opens a tab there.
    Machines,
}

/// The machine menu's note when no machine is linked yet (rule 1236).
pub const NO_MACHINES: &str = "No linked machines yet";

/// Ctrl+Shift+R's menu (rule 1236): the + menu's machine rows, each carrying its machine like there; with no machine
/// linked, one disabled note.
pub fn machine_menu_rows(machines: &[crate::machines::Machine]) -> Vec<(MenuRowData, String)> {
    let mut rows = machine_rows(machines);
    match rows.first_mut() {
        // The + menu draws a rule above its first machine; alone in its menu it needs none.
        Some((first, _)) => first.group = false,
        None => {
            let mut note = MenuRowData::plain(NO_MACHINES);
            note.icon = GLYPH_LINK;
            note.disabled = true;
            rows.push((note, String::new()));
        }
    }
    rows
}

/// The folder menu's first row: the folder picker, then a tab there (2026-10-27).
pub const CHOOSE_FOLDER: &str = "Choose a folder\u{2026}";

/// What a machine's row in the + menu carries instead of a folder (remote sessions PR 7): this mark, then the machine's
/// id. A NUL starts it, so no folder's path is ever read as one.
pub const MACHINE_ROW: &str = "\u{0}machine:";

/// The machine a + menu row opens a shell on, for a machine's row.
pub fn machine_of_row(path: &str) -> Option<&str> {
    path.strip_prefix(MACHINE_ROW)
}

/// The logo glyph of a machine's system (remote sessions PR 7); none for a system not known.
pub fn machine_logo(kind: Option<flare_tabs::tabs::Os>) -> Option<&'static str> {
    kind.map(|k| match k {
        flare_tabs::tabs::Os::Mac => GLYPH_LOGO_MAC,
        flare_tabs::tabs::Os::Pc => GLYPH_LOGO_PC,
    })
}

/// The + menu's machine rows (owner pick, 2026-11-12): one a linked machine, its system's logo and its name, after a
/// rule. A pick opens a shell tab on that machine, in the host's home folder; there is no submenu.
fn machine_rows(machines: &[crate::machines::Machine]) -> Vec<(MenuRowData, String)> {
    machines
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut row = MenuRowData::plain(&m.name);
            if let Some(g) = machine_logo(m.kind) {
                row.icon_cmd = glyph_path(g).to_string();
                row.filled_icon = true;
            } else {
                row.icon = GLYPH_LINK;
            }
            row.group = i == 0;
            (row, format!("{MACHINE_ROW}{}", m.id))
        })
        .collect()
}

/// What a remote folder's row in the + menu carries instead of a folder (rule 1213): this mark, the machine's id, a
/// NUL, then the host's folder. A NUL starts it, so no folder's path is ever read as one.
pub const REMOTE_FOLDER_ROW: &str = "\u{0}remote:";

/// The machine and the host folder a + menu row opens a remote tab in, for a remote folder's row.
pub fn remote_folder_of_row(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(REMOTE_FOLDER_ROW)?;
    let (machine, folder) = rest.split_once('\u{0}')?;
    (!machine.is_empty() && !folder.is_empty()).then_some((machine, folder))
}

/// The + menu's remote folders (rule 1213): the newest folders remote tabs opened, after this machine's, each with its
/// machine's logo before its name and the machine's name on its right. A machine no longer in `machines.json` drops its
/// folders from the list. No pin: a pin is this machine's.
fn remote_folder_rows(
    prefs: &PrefStore,
    machines: &[crate::machines::Machine],
) -> Vec<(MenuRowData, String)> {
    let listed: Vec<(RemoteRoot, &crate::machines::Machine)> = remote_roots(prefs)
        .into_iter()
        .filter_map(|r| {
            let m = machines.iter().find(|m| m.id == r.machine)?;
            Some((r, m))
        })
        .take(REMOTE_SHOWN)
        .collect();
    let paths: Vec<String> = listed.iter().map(|(r, _)| r.path.clone()).collect();
    let labels = recent_labels(&paths);
    listed
        .into_iter()
        .zip(labels)
        .enumerate()
        .map(|(i, ((r, m), l))| {
            let mut row = MenuRowData::plain(&l.label);
            if let Some(g) = machine_logo(m.kind) {
                row.icon_cmd = glyph_path(g).to_string();
                row.filled_icon = true;
            } else {
                row.icon = GLYPH_LINK;
            }
            row.hint = m.name.clone();
            row.group = i == 0;
            (
                row,
                format!("{REMOTE_FOLDER_ROW}{}\u{0}{}", r.machine, r.path),
            )
        })
        .collect()
}

/// What a starter folder's row says on its right: "user folder" for the user's own, `~\Documents` for one inside
/// it, nothing for one elsewhere (a Documents moved to another drive).
fn starter_hint(path: &str, home: &str) -> String {
    let (p, h) = (
        path.trim_end_matches(['\\', '/']),
        home.trim_end_matches(['\\', '/']),
    );
    if h.is_empty() {
        return String::new();
    }
    if p.eq_ignore_ascii_case(h) {
        return "user folder".to_string();
    }
    match p.get(..h.len()) {
        Some(head) if head.eq_ignore_ascii_case(h) && p[h.len()..].starts_with(['\\', '/']) => {
            format!("~{}", &p[h.len()..])
        }
        _ => String::new(),
    }
}

/// The folder menu (the sidebar's arrow and the strip's right-click): "Choose a folder…", then the pins, then the
/// newest unpinned recents. While fewer recents than the menu shows are known, `starters` (Documents, Downloads,
/// Desktop, the user folder: those that exist) fill the empty spots; a real recent pushes one out. Nothing to
/// list at all: a disabled note under the first row. Each row comes with the folder it opens ("" for the first
/// row and the note).
/// After them, one row per linked machine (remote sessions PR 7, [`machine_rows`]).
pub fn plus_menu_rows(
    prefs: &PrefStore,
    starters: &[String],
    home: &str,
    machines: &[crate::machines::Machine],
) -> Vec<(MenuRowData, String)> {
    let remote = remote_folder_rows(prefs, machines);
    let mut rows = folder_rows(prefs, starters, home, !remote.is_empty());
    rows.extend(remote);
    rows.extend(machine_rows(machines));
    rows
}

fn folder_rows(
    prefs: &PrefStore,
    starters: &[String],
    home: &str,
    has_remote: bool,
) -> Vec<(MenuRowData, String)> {
    let (pinned, recent) = (pinned_roots(prefs), recent_roots(prefs));
    let places = place_rows(&pinned, &recent);
    let fill = starter_fill(&pinned, &recent, starters, RECENT_SHOWN);
    let mut choose = MenuRowData::plain(CHOOSE_FOLDER);
    choose.icon = GLYPH_FOLDER;
    let mut rows = vec![(choose, String::new())];
    if places.is_empty() && fill.is_empty() {
        if has_remote {
            return rows;
        }
        let mut note = MenuRowData::plain("No recent folders");
        note.disabled = true;
        note.group = true;
        rows.push((note, String::new()));
        return rows;
    }
    // The labels are worked out over every listed folder, so a starter and a recent of one name both take
    // their parent along.
    let all: Vec<String> = places
        .iter()
        .map(|p| p.path.clone())
        .chain(fill.iter().cloned())
        .collect();
    let labels = recent_labels(&all);
    let pins = places.iter().map(|p| p.pinned);
    let starter_flags = std::iter::repeat_n(false, fill.len());
    for (i, (pinned, path)) in pins.chain(starter_flags).zip(all).enumerate() {
        let label = labels.get(i).map_or(path.as_str(), |l| l.label.as_str());
        let mut row = MenuRowData::plain(label);
        row.icon = GLYPH_FOLDER_FILL;
        row.filled_icon = true;
        row.trailing_icon = Some(GLYPH_PIN);
        row.trailing_filled = pinned;
        row.trailing_tip = if pinned { "Unpin" } else { "Pin to this menu" }.to_string();
        row.group = i == 0;
        if i >= places.len() {
            row.hint = starter_hint(&path, home);
        }
        rows.push((row, path));
    }
    rows
}

/// Toggles the pin of the row at `index`, then re-reads the list: the row climbs to the top while
/// the menu stands.
pub fn toggle_pin_in_menu(
    prefs: &PrefStore,
    rows: &mut Vec<(MenuRowData, String)>,
    index: usize,
    starters: &[String],
    home: &str,
    machines: &[crate::machines::Machine],
) {
    let Some((_, path)) = rows.get(index) else {
        return;
    };
    if path.is_empty() || machine_of_row(path).is_some() || remote_folder_of_row(path).is_some() {
        return;
    }
    toggle_pin(prefs, &path.clone());
    *rows = plus_menu_rows(prefs, starters, home, machines);
}

/// A remote tab's menu row after Close tab (remote sessions PR 7, rule 1192): the tab goes from this window and the
/// session goes on on its machine.
pub const HIDE_HERE: &str = "Hide here only";

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
    fn the_plus_menu_is_choose_then_pins_then_five_recents_or_a_disabled_note() {
        let prefs = PrefStore::memory();
        let empty = plus_menu_rows(&prefs, &[], "", &[]);
        assert_eq!(labels(&empty), [CHOOSE_FOLDER, "No recent folders"]);
        assert!(!empty[0].0.disabled);
        assert_eq!(empty[0].0.icon, GLYPH_FOLDER);
        assert_eq!(empty[0].0.trailing_icon, None);
        assert!(empty[1].0.disabled);
        for i in (0..8).rev() {
            remember_root(&prefs, &format!(r"C:\r\recent{i}"));
        }
        toggle_pin(&prefs, r"C:\p\pinA");
        toggle_pin(&prefs, r"C:\p\pinB");
        let rows = plus_menu_rows(&prefs, &[], "", &[]);
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0].0.label, CHOOSE_FOLDER);
        assert_eq!(rows[0].1, "");
        assert!(rows[1].0.trailing_filled && rows[2].0.trailing_filled);
        assert!(rows[3..].iter().all(|(r, _)| !r.trailing_filled));
        assert_eq!(rows[1].1, r"C:\p\pinA");
        // A rule between the first row and the folders.
        assert!(rows[1].0.group && rows[2..].iter().all(|(r, _)| !r.group));
        assert!(rows.iter().all(|(r, _)| !r.disabled));
    }

    /// Owner pick (remote sessions PR 7): one row per linked machine after the folders, its logo and its name, no pin
    /// and no submenu; its pick names the machine, never a folder.
    #[test]
    fn each_linked_machine_is_one_row_after_the_folders_with_its_logo_and_name() {
        use crate::machines::Machine;
        use flare_tabs::tabs::Os;
        let m = |id: &str, name: &str, kind: Option<Os>| Machine {
            id: id.into(),
            name: name.into(),
            kind,
            ssh: id.into(),
            user: None,
            flare: "flare".into(),
        };
        let machines = [
            m("mac", "Mac Mini", Some(Os::Mac)),
            m("pc", "Desk", Some(Os::Pc)),
            m("nas", "nas", None),
        ];
        let prefs = PrefStore::memory();
        remember_root(&prefs, r"C:\w\a");
        let mut rows = plus_menu_rows(&prefs, &[], "", &machines);
        assert_eq!(
            labels(&rows),
            [CHOOSE_FOLDER, "a", "Mac Mini", "Desk", "nas"]
        );
        let (mac, path) = &rows[2];
        assert_eq!(machine_of_row(path), Some("mac"));
        assert_eq!(mac.icon_cmd, glyph_path(GLYPH_LOGO_MAC));
        assert!(mac.filled_icon && mac.group && mac.trailing_icon.is_none() && !mac.flyout);
        assert_eq!(rows[3].0.icon_cmd, glyph_path(GLYPH_LOGO_PC));
        assert!(!rows[3].0.group);
        // A system not known draws no logo of either.
        assert!(rows[4].0.icon_cmd.is_empty());
        assert_eq!(rows[4].0.icon, GLYPH_LINK);
        // A folder's path is never a machine; a machine's row has no pin to toggle.
        assert_eq!(machine_of_row(r"C:\w\a"), None);
        toggle_pin_in_menu(&prefs, &mut rows, 2, &[], "", &machines);
        assert!(pinned_roots(&prefs).is_empty());
        // No folder known: the note, then the machines.
        let none = plus_menu_rows(&PrefStore::memory(), &[], "", &machines[..1]);
        assert_eq!(
            labels(&none),
            [CHOOSE_FOLDER, "No recent folders", "Mac Mini"]
        );
    }

    /// Rule 1213: a remote tab's folder is listed after this machine's, with its machine's logo before its name and the
    /// machine's name on its right, no pin; its row names the machine and the host folder. A machine no longer linked
    /// drops its folders from the list.
    #[test]
    fn remote_folders_follow_the_local_ones_with_their_machines_logo() {
        use crate::machines::Machine;
        use flare_settings::recent_roots::remember_remote_root;
        use flare_tabs::tabs::Os;
        let mac = Machine {
            id: "mac".into(),
            name: "Mac Mini".into(),
            kind: Some(Os::Mac),
            ssh: "mini".into(),
            user: None,
            flare: "flare".into(),
        };
        let prefs = PrefStore::memory();
        remember_root(&prefs, r"C:\w\a");
        remember_remote_root(&prefs, "gone", "/srv/old", false);
        remember_remote_root(&prefs, "mac", "/Users/max/proj", false);
        let mut rows = plus_menu_rows(&prefs, &[], "", std::slice::from_ref(&mac));
        assert_eq!(labels(&rows), [CHOOSE_FOLDER, "a", "proj", "Mac Mini"]);
        let (row, path) = &rows[2];
        assert_eq!(remote_folder_of_row(path), Some(("mac", "/Users/max/proj")));
        assert_eq!(machine_of_row(path), None);
        assert_eq!(row.icon_cmd, glyph_path(GLYPH_LOGO_MAC));
        assert!(row.filled_icon && row.group && row.trailing_icon.is_none());
        assert_eq!(row.hint, "Mac Mini");
        assert_eq!(remote_folder_of_row(r"C:\w\a"), None);
        // A remote folder's row has no pin to toggle.
        toggle_pin_in_menu(&prefs, &mut rows, 2, &[], "", std::slice::from_ref(&mac));
        assert!(pinned_roots(&prefs).is_empty());
        // Only remote folders known: no note, the remote folder, then the machine.
        let only = PrefStore::memory();
        remember_remote_root(&only, "mac", "/Users/max/proj", false);
        assert_eq!(
            labels(&plus_menu_rows(&only, &[], "", std::slice::from_ref(&mac))),
            [CHOOSE_FOLDER, "proj", "Mac Mini"]
        );
        // No machines linked: no remote folder.
        assert_eq!(
            labels(&plus_menu_rows(&prefs, &[], "", &[])),
            [CHOOSE_FOLDER, "a"]
        );
    }

    #[test]
    fn starters_fill_the_menu_until_real_recents_push_them_out() {
        let prefs = PrefStore::memory();
        let home = r"C:\Users\me";
        let starters: Vec<String> = [
            r"C:\Users\me\Documents",
            r"C:\Users\me\Downloads",
            r"D:\Desktop",
            home,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = plus_menu_rows(&prefs, &starters, home, &[]);
        assert_eq!(
            labels(&rows),
            [CHOOSE_FOLDER, "Documents", "Downloads", "Desktop", "me"]
        );
        let hints: Vec<&str> = rows.iter().map(|(r, _)| r.hint.as_str()).collect();
        assert_eq!(
            hints,
            ["", r"~\Documents", r"~\Downloads", "", "user folder"]
        );
        assert_eq!(rows[1].1, r"C:\Users\me\Documents");
        assert!(rows[1].0.group);
        // Two recents: they come first and the last two starters give way.
        remember_root(&prefs, r"C:\work\b");
        remember_root(&prefs, r"C:\work\a");
        let rows = plus_menu_rows(&prefs, &starters, home, &[]);
        assert_eq!(
            labels(&rows),
            [CHOOSE_FOLDER, "a", "b", "Documents", "Downloads", "Desktop"]
        );
        assert_eq!(rows[1].0.hint, "");
        // A starter opened once is a recent: listed once, as a recent.
        remember_root(&prefs, r"C:\Users\me\Downloads");
        let rows = plus_menu_rows(&prefs, &starters, home, &[]);
        assert_eq!(
            labels(&rows),
            [CHOOSE_FOLDER, "Downloads", "a", "b", "Documents", "Desktop"]
        );
        assert_eq!(rows[1].0.hint, "");
        // Pinning a starter makes it a pin.
        let mut rows = rows;
        toggle_pin_in_menu(&prefs, &mut rows, 4, &starters, home, &[]);
        assert_eq!(
            labels(&rows),
            [
                CHOOSE_FOLDER,
                "Documents",
                "Downloads",
                "a",
                "b",
                "Desktop",
                "me"
            ]
        );
        assert!(rows[1].0.trailing_filled);
    }

    #[test]
    fn pinning_in_the_menu_moves_the_row_up_and_keeps_the_menu() {
        let prefs = PrefStore::memory();
        for p in [r"C:\x\c", r"C:\x\b", r"C:\x\a"] {
            remember_root(&prefs, p);
        }
        let mut rows = plus_menu_rows(&prefs, &[], "", &[]);
        assert_eq!(
            rows.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
            ["", r"C:\x\a", r"C:\x\b", r"C:\x\c"]
        );
        // The first row has no pin: toggling it does nothing.
        toggle_pin_in_menu(&prefs, &mut rows, 0, &[], "", &[]);
        assert!(pinned_roots(&prefs).is_empty());
        toggle_pin_in_menu(&prefs, &mut rows, 3, &[], "", &[]);
        assert_eq!(
            rows.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
            ["", r"C:\x\c", r"C:\x\a", r"C:\x\b"]
        );
        assert_eq!(pinned_roots(&prefs), [r"C:\x\c"]);
        assert_eq!(rows[1].0.trailing_tip, "Unpin");
        assert_eq!(rows[2].0.trailing_tip, "Pin to this menu");
        assert_eq!(rows[3].0.trailing_tip, "Pin to this menu");
        // And back: unpinning drops it to the recents' place.
        toggle_pin_in_menu(&prefs, &mut rows, 1, &[], "", &[]);
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
            GLYPH_LOGO_MAC,
            GLYPH_LOGO_PC,
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
        assert_eq!(
            labels(&plus_menu_rows(&prefs, &[], "", &[])),
            [CHOOSE_FOLDER, "No recent folders"]
        );
    }
}
