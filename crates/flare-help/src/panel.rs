//! The help panel model (M7 Task 28). Ported from Prism Terminal's `HelpPanel.tsx`: the rows the
//! panel draws, the keys it answers, the copy rule and the copied state. It shows and copies, and
//! nothing else: it holds no handle to a shell, and the host does the clipboard.

use crate::entries::{HELP_CATEGORIES, blankless, help_for, is_key_press};
use crate::search::{HelpSearchOptions, search_help};
use crate::shells::{shell_of_shell_id, variants_in};
use crate::types::{HelpEntry, HelpShell, HelpShellChoice};

/// How many entries are drawn at first, and how many more each time the end comes into view.
pub const FIRST_PAGE: usize = 40;
/// How many more entries one near-end or key move draws.
pub const NEXT_PAGE: usize = 60;
/// A search answers with at most this many.
pub const MAX_RESULTS: usize = 50;
/// How long a copied row wears its check.
pub const COPIED_MS: u64 = 1400;
/// How far a press may travel and still be a click, not a drag.
pub const DRAG_PX: f64 = 4.0;
/// The slide in and out (the host's timing, kept here with the rest of the panel's numbers).
pub const IN_MS: u64 = 460;
pub const OUT_MS: u64 = 220;
pub const EMPTY_TEXT: &str = "Nothing matches. Say it another way (what you want to DO, such as \"delete a folder\"), or try another shell.";
pub const PLACEHOLDER: &str = "Try: find big files, what is using port 3000";
pub const SEARCH_LABEL: &str =
    "Search commands. Up and Down move through the results, Enter copies the highlighted command.";

/// One row of the list: an entry's own command (`n` 0) or one of its variants (`n` 1 and up,
/// counted over every variant so the id is the same whichever shell draws it).
#[derive(Clone, Debug)]
pub struct HelpRow {
    pub entry: &'static HelpEntry,
    pub n: usize,
    /// The left column: the task, or the variant's label.
    pub name: String,
    /// What is shown and copied (`blankless`).
    pub command: String,
}

impl HelpRow {
    fn key(&self) -> String {
        format!("{}#{}", self.entry.id, self.n)
    }
}

/// The keys the host forwards to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Escape,
    Down,
    Up,
    PageDown,
    PageUp,
    Enter,
}

/// What a key or a click asks the host to do.
#[derive(Debug, PartialEq, Eq)]
pub enum PanelOut {
    None,
    Close,
    /// Put `text` on the clipboard, then report back with `copy_result` under the same `key`.
    Copy {
        key: String,
        text: String,
    },
    /// Said to a screen reader and shown in the status line.
    Said(String),
}

/// The list in the order it is drawn. A search is the shell's best entries; browsing is every
/// entry of the shell grouped by category, in the panel's category order.
fn rows_for(shell: HelpShellChoice, query: &str) -> Vec<HelpRow> {
    let entries: Vec<&'static HelpEntry> = if query.trim().is_empty() {
        let mine = help_for(shell);
        let mut out = Vec::new();
        for (category, _) in HELP_CATEGORIES {
            out.extend(mine.iter().copied().filter(|e| e.category == category));
        }
        out
    } else {
        let opts = HelpSearchOptions {
            shell: Some(help_shell_of(shell)),
            category: None,
            limit: Some(MAX_RESULTS),
        };
        search_help(crate::entries::all_help(), query, &opts)
            .into_iter()
            .map(|h| h.entry)
            .collect()
    };
    let mut rows = Vec::new();
    for entry in entries {
        let names: Vec<&str> = entry.placeholders.keys().map(String::as_str).collect();
        rows.push(HelpRow {
            entry,
            n: 0,
            name: entry.task.clone(),
            command: blankless(&entry.command, &names),
        });
        for (i, v) in entry.variants.iter().enumerate() {
            if !variants_in(std::slice::from_ref(v), shell).is_empty() {
                rows.push(HelpRow {
                    entry,
                    n: i + 1,
                    name: v.label.clone(),
                    command: blankless(&v.command, &names),
                });
            }
        }
    }
    rows
}

fn help_shell_of(shell: HelpShellChoice) -> HelpShell {
    match shell {
        HelpShellChoice::Powershell => HelpShell::Powershell,
        HelpShellChoice::Cmd => HelpShell::Cmd,
        HelpShellChoice::Bash => HelpShell::Bash,
    }
}

/// The panel's state: which shell chip is selected, the question, the marked row, how many
/// entries are drawn, and the copied and said messages.
#[derive(Clone, Debug)]
pub struct PanelState {
    chip: HelpShellChoice,
    query: String,
    active: usize,
    marked: bool,
    limit: usize,
    /// The key of the last copied row, whether it worked, and the time it was copied at.
    copied: Option<(String, bool, u64)>,
    said: String,
    rows: Vec<HelpRow>,
}

impl PanelState {
    pub fn new(shell: HelpShellChoice) -> Self {
        Self {
            chip: shell,
            query: String::new(),
            active: 0,
            marked: false,
            limit: FIRST_PAGE,
            copied: None,
            said: String::new(),
            rows: rows_for(shell, ""),
        }
    }

    /// Each opening starts on the shell it is given, at the top, with an empty question.
    pub fn open(&mut self, shell: HelpShellChoice) {
        self.chip = shell;
        self.query.clear();
        self.reset_list();
    }

    /// A new question is a new list: back to the top, the first page, nothing marked.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.reset_list();
    }

    /// A shell chip picked by hand. The question stays; the list starts again from the top.
    pub fn pick_chip(&mut self, shell: HelpShellChoice) {
        self.chip = shell;
        self.reset_list();
    }

    fn reset_list(&mut self) {
        self.active = 0;
        self.marked = false;
        self.limit = FIRST_PAGE;
        self.rows = rows_for(self.chip, &self.query);
    }

    /// Keys. Chords (Ctrl, Alt or Meta held) are the host's and are left untouched.
    pub fn key(&mut self, k: PanelKey, ctrl_alt_meta: bool) -> PanelOut {
        if ctrl_alt_meta {
            return PanelOut::None;
        }
        match k {
            PanelKey::Escape => PanelOut::Close,
            PanelKey::Down | PanelKey::Up | PanelKey::PageDown | PanelKey::PageUp => {
                if self.rows.is_empty() {
                    return PanelOut::None;
                }
                let step: isize = match k {
                    PanelKey::Down => 1,
                    PanelKey::Up => -1,
                    PanelKey::PageDown => 5,
                    _ => -5,
                };
                let last = self.rows.len() as isize - 1;
                // The first press marks the row the cursor is already on; later presses move.
                let next = if self.marked {
                    (self.active as isize + step).clamp(0, last) as usize
                } else {
                    self.active
                };
                self.marked = true;
                if next + 5 > self.limit {
                    self.limit = next + NEXT_PAGE;
                }
                self.active = next;
                PanelOut::None
            }
            PanelKey::Enter => {
                let Some(row) = self.rows.get(self.active) else {
                    return PanelOut::None;
                };
                if is_key_press(&row.command) {
                    self.said = format!("{} is a key to press, not text to copy", row.command);
                    PanelOut::None
                } else {
                    PanelOut::Copy {
                        key: row.key(),
                        text: row.command.clone(),
                    }
                }
            }
        }
    }

    /// A press on row `i` makes it the cursor's row.
    pub fn pointer_down_on(&mut self, i: usize) {
        if i < self.rows.len() {
            self.active = i;
        }
    }

    /// The pointer moved over the list: the keyboard's mark goes.
    pub fn pointer_moved_over_list(&mut self) {
        self.marked = false;
    }

    /// A click on row `i`. A key to press copies nothing, and neither does a press that moved
    /// more than `DRAG_PX` (a selection, not a click).
    pub fn click_row(&mut self, i: usize, moved_px: f64) -> PanelOut {
        let Some(row) = self.rows.get(i) else {
            return PanelOut::None;
        };
        self.active = i;
        if is_key_press(&row.command) || moved_px > DRAG_PX {
            return PanelOut::None;
        }
        PanelOut::Copy {
            key: row.key(),
            text: row.command.clone(),
        }
    }

    /// The host's answer to a `Copy`: whether the clipboard took it, and when.
    pub fn copy_result(&mut self, key: &str, command: &str, ok: bool, now_ms: u64) {
        self.copied = Some((key.to_string(), ok, now_ms));
        self.said = if ok {
            format!("Copied {command}")
        } else {
            "Could not copy to the clipboard".to_string()
        };
    }

    /// When the copied check was shown (the host's clock), while it still is.
    pub fn copied_since(&self) -> Option<u64> {
        self.copied.as_ref().map(|(_, _, at)| *at)
    }

    /// The copied check leaves after `COPIED_MS`, with the message it came with.
    pub fn tick(&mut self, now_ms: u64) {
        if let Some((_, _, at)) = &self.copied
            && now_ms.saturating_sub(*at) >= COPIED_MS
        {
            self.copied = None;
            self.said.clear();
        }
    }

    /// The end of the drawn list came into view: draw the next page.
    pub fn near_end(&mut self) {
        self.limit += NEXT_PAGE;
    }

    /// The count beside the chips.
    pub fn count_text(&self) -> String {
        if self.searching() {
            let n = self.rows.len();
            if n == 0 {
                return "No results".to_string();
            }
            let plus = if n == MAX_RESULTS { "+" } else { "" };
            let word = if n == 1 { "result" } else { "results" };
            format!("{n}{plus} {word}")
        } else {
            format!("{} commands", self.rows.len())
        }
    }

    /// The rows drawn so far (the first `limit`).
    pub fn drawn(&self) -> &[HelpRow] {
        &self.rows[..self.limit.min(self.rows.len())]
    }

    /// Every row of the current list.
    pub fn rows(&self) -> &[HelpRow] {
        &self.rows
    }

    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn chip(&self) -> HelpShellChoice {
        self.chip
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn marked(&self) -> bool {
        self.marked
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The copied row's key and whether the copy worked, while its check is up.
    pub fn copied(&self) -> Option<(&str, bool)> {
        self.copied.as_ref().map(|(k, ok, _)| (k.as_str(), *ok))
    }

    /// What a screen reader is told, and the status line shows.
    pub fn said(&self) -> &str {
        &self.said
    }

    /// The category heading above row `i`, when browsing and the category changes there.
    pub fn heading_for(&self, i: usize) -> Option<&'static str> {
        if self.searching() || i >= self.rows.len() {
            return None;
        }
        let category = self.rows[i].entry.category;
        let changed = i == 0 || self.rows[i - 1].entry.category != category;
        if !changed {
            return None;
        }
        HELP_CATEGORIES
            .iter()
            .find(|(c, _)| *c == category)
            .map(|(_, name)| *name)
    }

    /// The id a screen reader is pointed at, only while a row is marked by the keyboard.
    pub fn active_id(&self) -> Option<String> {
        if !self.marked {
            return None;
        }
        self.rows
            .get(self.active)
            .map(|r| format!("help-entry-{}-{}", r.entry.id, r.n))
    }

    /// The stripe follows the drawn order, so it never restarts under a heading.
    pub fn stripe(i: usize) -> bool {
        i % 2 == 1
    }
}

/// The shell the panel opens on: the language of the terminal tab in front, else the chip the
/// person picked by hand, else PowerShell.
pub fn front_shell(front_tab_shell_id: Option<&str>, picked: Option<&str>) -> HelpShellChoice {
    match front_tab_shell_id {
        Some(id) => shell_of_shell_id(Some(id)),
        None => match picked {
            Some("cmd") => HelpShellChoice::Cmd,
            Some("bash") => HelpShellChoice::Bash,
            _ => HelpShellChoice::Powershell,
        },
    }
}
