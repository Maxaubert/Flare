//! The closed row tables of the Settings pages (M7 Task 10).
//!
//! Source: Prism Terminal v0.55.6's `core/renderer/settings/{options,markOptions,helpOptions,
//! diagnosticsOptions,dictationOptions}.ts` and `components/settings/appOptions.ts`. The label,
//! subtext and keywords of every row are copied from there (Appendix C of the M7 plan); the keys
//! are Flare's own (`flare.*`, decision O10). The Explorer row reads "Add to the Explorer menu"
//! (D5). Only the See-through row's subtext breaks the eight-word rule, on purpose (O8).
//!
//! Every row carries the storage keys its module reads and writes. `ROW_ORDER` is the draw order
//! of the 47 rows; within each section the order follows the tables.

/// What control a row draws. `Registry` is the Explorer verb (read back from Windows, M9);
/// `Info` is a row that stores nothing (the version).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Choice,
    Switch,
    Range,
    Colour,
    Theme,
    Key,
    Manager,
    Action,
    Registry,
    Info,
}

/// The Prism Terminal list a row came from (its record, not a behaviour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Terminal,
    Marks,
    Dictation,
    Help,
    Diagnostics,
    App,
}

/// One row of a Settings page: its words, where it is drawn, and what it stores.
#[derive(Clone, Copy, Debug)]
pub struct RowDef {
    pub id: &'static str,
    pub label: &'static str,
    pub sub: &'static str,
    pub section: &'static str,
    pub page: &'static str,
    pub icon: &'static str,
    pub keywords: &'static str,
    pub keys: &'static [&'static str],
    pub kind: RowKind,
    pub source: Source,
    pub only_nvidia: bool,
}

/// The rows of the terminal list, in PT's order (the marks row is separate; see `MARK_OPTIONS`).
pub const TERMINAL_OPTIONS: [RowDef; 13] = [
    RowDef {
        id: "term-shell",
        label: "Default shell",
        sub: "New terminals start with this shell.",
        section: "shell",
        page: "terminal",
        icon: "shell",
        keywords: "powershell pwsh cmd command prompt bash wsl",
        keys: &["flare.term.shell"],
        kind: RowKind::Choice,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "term-font-family",
        label: "Terminal font",
        sub: "The typeface inside every terminal.",
        section: "text",
        page: "terminal",
        icon: "font",
        keywords: "typeface face monospace cascadia consolas",
        keys: &["flare.term.font"],
        kind: RowKind::Choice,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "term-font",
        label: "Terminal text size",
        sub: "Text size for every terminal.",
        section: "text",
        page: "terminal",
        icon: "size",
        keywords: "font zoom bigger smaller scale",
        keys: &["flare.term.fontPct"],
        kind: RowKind::Choice,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-indicator",
        label: "Agent working indicator",
        sub: "A line under the tab while it works.",
        section: "marks",
        page: "agents",
        icon: "working",
        keywords: "claude codex busy bar minimal full mark ring spinner solid fill",
        keys: &["flare.term.agentIndicator"],
        kind: RowKind::Choice,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-done-on",
        label: "Mark tabs when an agent finishes",
        sub: "Stays until you open the tab.",
        section: "marks",
        page: "agents",
        icon: "done",
        keywords: "finished complete done indicator",
        keys: &["flare.term.agentDoneOn"],
        kind: RowKind::Switch,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-question-on",
        label: "Mark tabs when an agent asks",
        sub: "Stays until you answer or open it.",
        section: "marks",
        page: "agents",
        icon: "ask",
        keywords: "question waiting answer indicator",
        keys: &["flare.term.agentQuestionOn"],
        kind: RowKind::Switch,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-failed-on",
        label: "Mark tabs when an agent fails",
        sub: "When it stops on an error.",
        section: "marks",
        page: "agents",
        icon: "fail",
        keywords: "failed error crash stopped indicator",
        keys: &["flare.term.agentFailedOn"],
        kind: RowKind::Switch,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-hooks",
        label: "Exact status from Claude Code",
        sub: "Applies to terminals opened after a change.",
        section: "claude",
        page: "agents",
        icon: "hook",
        keywords: "hooks plugin status anthropic",
        keys: &["flare.term.agentHooks"],
        kind: RowKind::Switch,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "term-theme",
        label: "Terminal theme",
        sub: "Colours of the terminal and the window.",
        section: "theme",
        page: "appearance",
        icon: "brush",
        keywords: "colors palette scheme dark light preset custom",
        keys: &["flare.term.theme", "flare.term.custom"],
        kind: RowKind::Theme,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "term-acrylic",
        label: "See through window",
        sub: "How much of the desktop shows behind every surface.",
        section: "theme",
        page: "appearance",
        icon: "glass",
        keywords: "acrylic transparent glass blur mica see through",
        keys: &["flare.term.seeThrough"],
        kind: RowKind::Range,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-color",
        label: "Agent working colour",
        sub: "Follows the accent.",
        section: "colours",
        page: "agents",
        icon: "working",
        keywords: "color indicator busy",
        keys: &["flare.term.agentColor"],
        kind: RowKind::Colour,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-done-color",
        label: "Agent finished colour",
        sub: "Follows the theme green.",
        section: "colours",
        page: "agents",
        icon: "done",
        keywords: "color indicator complete",
        keys: &["flare.term.agentDoneColor"],
        kind: RowKind::Colour,
        source: Source::Terminal,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-question-color",
        label: "Agent question colour",
        sub: "Follows the theme.",
        section: "colours",
        page: "agents",
        icon: "ask",
        keywords: "color indicator waiting",
        keys: &["flare.term.agentQuestionColor"],
        kind: RowKind::Colour,
        source: Source::Terminal,
        only_nvidia: false,
    },
];

/// The tab marks' own row (drawn only where the marks live).
pub const MARK_OPTIONS: [RowDef; 1] = [RowDef {
    id: "agent-rainbow",
    label: "Rainbow finished mark",
    sub: "The colours of the app icon, flowing.",
    section: "marks",
    page: "agents",
    icon: "rainbow",
    keywords: "colors gradient finished done icon",
    keys: &["flare.term.agentRainbow"],
    kind: RowKind::Switch,
    source: Source::Marks,
    only_nvidia: false,
}];

/// The dictation rows. The GPU row is offered only with an NVIDIA adapter.
pub const DICTATION_OPTIONS: [RowDef; 10] = [
    RowDef {
        id: "dictation-enabled",
        label: "Use dictation",
        sub: "Speech to text on this PC, never sent.",
        section: "dictation",
        page: "dictation",
        icon: "mic",
        keywords: "voice speech whisper talk dictate",
        keys: &["flare.dictation.enabled"],
        kind: RowKind::Switch,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-mode",
        label: "Key behaviour",
        sub: "Runs while the key is held.",
        section: "listening",
        page: "dictation",
        icon: "hand",
        keywords: "push talk hold toggle trigger",
        keys: &["flare.dictation.mode"],
        kind: RowKind::Choice,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-hotkey",
        label: "Dictation key",
        sub: "Text is pasted at the cursor.",
        section: "listening",
        page: "dictation",
        icon: "key",
        keywords: "hotkey shortcut right alt",
        keys: &["flare.dictation.hotkey"],
        kind: RowKind::Key,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-mic",
        label: "Microphone",
        sub: "Test it to see the level.",
        section: "listening",
        page: "dictation",
        icon: "mic",
        keywords: "input device level headset",
        keys: &["flare.dictation.mic"],
        kind: RowKind::Choice,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-language",
        label: "Spoken language",
        sub: "The language you speak.",
        section: "listening",
        page: "dictation",
        icon: "globe",
        keywords: "locale english norwegian auto detect",
        keys: &["flare.dictation.language"],
        kind: RowKind::Choice,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-pause-media",
        label: "Pause media while dictating",
        sub: "Resumes when you stop.",
        section: "while",
        page: "dictation",
        icon: "pause",
        keywords: "music video audio",
        keys: &["flare.dictation.pauseMedia"],
        kind: RowKind::Switch,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-sounds",
        label: "Play start and stop sounds",
        sub: "A short sound as the microphone opens.",
        section: "while",
        page: "dictation",
        icon: "sound",
        keywords: "beep chime audio",
        keys: &["flare.dictation.sounds"],
        kind: RowKind::Switch,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-model",
        label: "Speech models",
        sub: "Downloaded once, kept on this PC.",
        section: "models",
        page: "dictation",
        icon: "download",
        keywords: "whisper parakeet download model",
        keys: &["flare.dictation.model"],
        kind: RowKind::Manager,
        source: Source::Dictation,
        only_nvidia: false,
    },
    RowDef {
        id: "dictation-gpu",
        label: "GPU acceleration",
        sub: "Large models answer in under a second.",
        section: "gpu",
        page: "dictation",
        icon: "chip",
        keywords: "cuda nvidia graphics card fast",
        keys: &[],
        kind: RowKind::Manager,
        source: Source::Dictation,
        only_nvidia: true,
    },
    RowDef {
        id: "dictation-anywhere",
        label: "Dictate from any app",
        sub: "The key works with another app in front.",
        section: "anywhere",
        page: "dictation",
        icon: "mic",
        keywords: "global anywhere outside background other app widget overlay send session hotkey ctrl win",
        keys: &["flare.dictation.anywhere"],
        kind: RowKind::Switch,
        source: Source::Dictation,
        only_nvidia: false,
    },
];

/// Command help's one row.
pub const HELP_OPTIONS: [RowDef; 1] = [RowDef {
    id: "help-enabled",
    label: "Command help",
    sub: "Find a command by describing it.",
    section: "help",
    page: "terminal",
    icon: "search",
    keywords: "cheat sheet commands lookup shell",
    keys: &["flare.help.enabled"],
    kind: RowKind::Switch,
    source: Source::Help,
    only_nvidia: false,
}];

/// The diagnostics rows. None stores a key: the log flag lives in `diag.json` (M7 Task 29).
pub const DIAGNOSTICS_OPTIONS: [RowDef; 3] = [
    RowDef {
        id: "diag-verbose",
        label: "Detailed logging",
        sub: "Records every call, for tracking down a problem.",
        section: "diagnostics",
        page: "diagnostics",
        icon: "log",
        keywords: "verbose debug log trace",
        keys: &[],
        kind: RowKind::Switch,
        source: Source::Diagnostics,
        only_nvidia: false,
    },
    RowDef {
        id: "diag-folder",
        label: "Log files",
        sub: "Kept on this PC, never sent.",
        section: "diagnostics",
        page: "diagnostics",
        icon: "folder",
        keywords: "folder open logs diagnostics jsonl",
        keys: &[],
        kind: RowKind::Action,
        source: Source::Diagnostics,
        only_nvidia: false,
    },
    RowDef {
        id: "diag-mark",
        label: "Mark a problem",
        sub: "Stamps this moment in the log.",
        section: "diagnostics",
        page: "diagnostics",
        icon: "flag",
        keywords: "stall freeze hang slow report",
        keys: &[],
        kind: RowKind::Action,
        source: Source::Diagnostics,
        only_nvidia: false,
    },
];

/// This app's own rows, in draw order (section by section, as the pages draw them).
pub const APP_OPTIONS: [RowDef; 19] = [
    RowDef {
        id: "tab-layout",
        label: "Tab position",
        sub: "Tabs along the top, or down the side.",
        section: "window",
        page: "appearance",
        icon: "sidebar",
        keywords: "vertical horizontal sidebar side left strip top",
        keys: &[
            "flare.window.tabLayout",
            "flare.window.sidebar",
            "flare.window.sidebarWidth",
        ],
        kind: RowKind::Choice,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "tab-switch",
        label: "Tab switching",
        sub: "Which tab comes next when you switch.",
        section: "window",
        page: "appearance",
        icon: "key",
        keywords: "ctrl tab mru recent order cycle switch next previous last used",
        keys: &["flare.window.tabSwitch"],
        kind: RowKind::Choice,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "window-edges",
        label: "Panel edges",
        sub: "Lines between panels and around the window.",
        section: "window",
        page: "appearance",
        icon: "edges",
        keywords: "border lines hairline outline faint solid",
        keys: &["flare.window.edges"],
        kind: RowKind::Choice,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "window-background",
        label: "Background colour",
        sub: "Behind the text in window and terminal.",
        section: "theme",
        page: "appearance",
        icon: "viewer",
        keywords: "ground backdrop color transparent alpha opacity",
        keys: &["flare.window.background"],
        kind: RowKind::Colour,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "window-secondary",
        label: "Secondary colour",
        sub: "The sidebar and title bar beside the terminal.",
        section: "theme",
        page: "appearance",
        icon: "brush",
        keywords: "sidebar title bar panel surface chrome secondary color vertical tabs",
        keys: &["flare.window.secondary"],
        kind: RowKind::Colour,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "window-accent",
        label: "Accent colour",
        sub: "Highlights, the active tab and selection.",
        section: "theme",
        page: "appearance",
        icon: "accent",
        keywords: "highlight color selection",
        keys: &["flare.window.accent"],
        kind: RowKind::Colour,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "newtab-mode",
        label: "Folder for new tabs",
        sub: "Opens in your user folder.",
        section: "opening",
        page: "terminal",
        icon: "newtab",
        keywords: "folder home directory start new tab choose",
        keys: &["flare.newtab.mode", "flare.newtab.folder"],
        kind: RowKind::Action,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "explorer-verb",
        label: "Add to the Explorer menu",
        sub: "Open in Flare, on every folder.",
        section: "opening",
        page: "terminal",
        icon: "menu",
        keywords: "context menu right click open here",
        keys: &[],
        kind: RowKind::Registry,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-keys",
        label: "Agent hotkeys",
        sub: "Spare shell keys open the sidebar, switch tabs.",
        section: "keys",
        page: "terminal",
        icon: "key",
        keywords: "shortcuts hotkeys keyboard bindings ctrl s page up page down sidebar agent claude codex keys",
        keys: &["flare.window.agentKeys"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-guard",
        label: "Keep agent sessions open",
        sub: "Keys never clear or close a running agent.",
        section: "keys",
        page: "terminal",
        icon: "key",
        keywords: "ctrl c ctrl d exit close quit clear input interrupt claude codex agent session keys protect",
        keys: &["flare.window.agentGuard"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "right-click-reports",
        label: "Send right clicks to programs",
        sub: "Off keeps right clicks for the menu.",
        section: "keys",
        page: "terminal",
        icon: "key",
        keywords: "right click mouse paste menu context claude codex agent clicks program mouse mode",
        keys: &["flare.window.rightClickReports"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "open-previews",
        label: "Open previews in Flare",
        sub: "Pictures and text open beside the terminal.",
        section: "links",
        page: "terminal",
        icon: "viewer",
        keywords: "link click path picture image text file preview panel side open",
        keys: &["flare.window.openPreviews"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "wrap-previews",
        label: "Wrap long lines in previews",
        sub: "Long lines continue on the next row.",
        section: "links",
        page: "terminal",
        icon: "viewer",
        keywords: "wrap long lines text file preview panel side horizontal scroll fit width",
        keys: &["flare.window.wrapPreviews"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "agent-eval",
        label: "Let agents run scripts in pages",
        sub: "Agents may run code in their web panel.",
        section: "links",
        page: "terminal",
        icon: "viewer",
        keywords: "agent claude browser web page script javascript eval run code panel",
        keys: &["flare.web.agentEval"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "app-network",
        label: "Let mini apps use the network",
        sub: "Pages Claude writes may load from the web.",
        section: "links",
        page: "terminal",
        icon: "viewer",
        keywords: "agent claude mini app html page chart diagram network internet web online fetch",
        keys: &["flare.web.appNetwork"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "dev-preview",
        label: "Offer previews of local servers",
        sub: "A Preview chip for sites you start.",
        section: "links",
        page: "terminal",
        icon: "viewer",
        keywords: "localhost 127.0.0.1 dev server vite npm port web preview chip browser panel site",
        keys: &["flare.window.devPreview"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "taskbar-badge",
        label: "Count marked tabs on the taskbar",
        sub: "A number on the taskbar button.",
        section: "marks",
        page: "agents",
        icon: "taskbar",
        keywords: "count number badge overlay",
        keys: &["flare.window.taskbarBadge"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "notify-ask",
        label: "Notify me when an agent asks",
        sub: "A notification when a question waits.",
        section: "notify",
        page: "agents",
        icon: "ask",
        keywords: "notification toast question alert agent claude codex asks windows banner",
        keys: &["flare.window.notifyAsk"],
        kind: RowKind::Switch,
        source: Source::App,
        only_nvidia: false,
    },
    RowDef {
        id: "app-version",
        label: "Version",
        sub: "The version you are running.",
        section: "about",
        page: "about",
        icon: "version",
        keywords: "update release number",
        keys: &[],
        kind: RowKind::Info,
        source: Source::App,
        only_nvidia: false,
    },
];

/// The 47 row ids in draw order (Appendix C, the `#` column).
pub const ROW_ORDER: [&str; 47] = [
    "tab-layout",
    "tab-switch",
    "window-edges",
    "term-theme",
    "term-acrylic",
    "window-background",
    "window-secondary",
    "window-accent",
    "term-shell",
    "newtab-mode",
    "explorer-verb",
    "agent-keys",
    "agent-guard",
    "right-click-reports",
    "open-previews",
    "wrap-previews",
    "agent-eval",
    "app-network",
    "dev-preview",
    "term-font-family",
    "term-font",
    "help-enabled",
    "agent-indicator",
    "agent-done-on",
    "agent-rainbow",
    "agent-question-on",
    "agent-failed-on",
    "taskbar-badge",
    "agent-hooks",
    "agent-color",
    "agent-done-color",
    "agent-question-color",
    "notify-ask",
    "dictation-enabled",
    "dictation-mode",
    "dictation-hotkey",
    "dictation-mic",
    "dictation-language",
    "dictation-pause-media",
    "dictation-sounds",
    "dictation-model",
    "dictation-gpu",
    "dictation-anywhere",
    "diag-verbose",
    "diag-folder",
    "diag-mark",
    "app-version",
];

/// The row with this id, from any table.
pub fn row(id: &str) -> Option<&'static RowDef> {
    TERMINAL_OPTIONS
        .iter()
        .chain(MARK_OPTIONS.iter())
        .chain(DICTATION_OPTIONS.iter())
        .chain(HELP_OPTIONS.iter())
        .chain(DIAGNOSTICS_OPTIONS.iter())
        .chain(APP_OPTIONS.iter())
        .find(|r| r.id == id)
}

/// Every row, in `ROW_ORDER`.
pub fn all_rows() -> impl Iterator<Item = &'static RowDef> {
    ROW_ORDER.iter().filter_map(|id| row(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy_rules::{copy_problem, label_problem, sub_too_long};
    use crate::icons::is_setting_icon;
    use crate::pages::PAGES;
    use crate::sections::section_title;
    use std::collections::BTreeSet;

    /// The rows in the order the pages draw them: the terminal table with the marks row after
    /// `agent-done-on`, then dictation, help, diagnostics and this app's rows.
    fn table_order() -> Vec<&'static RowDef> {
        let mut out: Vec<&'static RowDef> = Vec::new();
        for r in TERMINAL_OPTIONS.iter() {
            out.push(r);
            if r.id == "agent-done-on" {
                out.extend(MARK_OPTIONS.iter());
            }
        }
        out.extend(DICTATION_OPTIONS.iter());
        out.extend(HELP_OPTIONS.iter());
        out.extend(DIAGNOSTICS_OPTIONS.iter());
        out.extend(APP_OPTIONS.iter());
        out
    }

    fn is_app(id: &str) -> bool {
        APP_OPTIONS.iter().any(|a| a.id == id)
    }

    /// The storage of a row as the snapshot writes it: its keys joined by `+`, or `registry`.
    fn store_text(r: &RowDef) -> String {
        if r.kind == RowKind::Registry {
            return "registry".to_string();
        }
        r.keys.join("+")
    }

    fn store_line(r: &RowDef) -> String {
        format!("{}={}", r.id, store_text(r))
    }

    #[test]
    fn the_tables_name_the_46_rows_in_row_order() {
        let ids: Vec<&str> = table_order().iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 47);
        let mut sorted_tables = ids.clone();
        sorted_tables.sort_unstable();
        let mut sorted_order: Vec<&str> = ROW_ORDER.to_vec();
        sorted_order.sort_unstable();
        assert_eq!(sorted_tables, sorted_order);
        let distinct: BTreeSet<&str> = ROW_ORDER.iter().copied().collect();
        assert_eq!(distinct.len(), 47);
        let drawn: Vec<&str> = all_rows().map(|r| r.id).collect();
        assert_eq!(drawn, ROW_ORDER.to_vec());
    }

    #[test]
    fn each_core_option_has_unique_keys_under_flare_term() {
        assert_eq!(TERMINAL_OPTIONS.len(), 13);
        let mut keys: Vec<&str> = Vec::new();
        for r in TERMINAL_OPTIONS.iter().chain(MARK_OPTIONS.iter()) {
            for k in r.keys {
                assert!(k.starts_with("flare.term."), "{} has key {k}", r.id);
                keys.push(k);
            }
        }
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }

    #[test]
    fn there_is_no_opacity_confirm_close_tab_width_or_tab_style_row() {
        for id in ["term-opacity", "confirm-close", "tab-width", "tab-style"] {
            assert!(row(id).is_none(), "{id} must not be a row");
        }
        for r in all_rows() {
            assert!(!r.keys.contains(&"flare.term.opacity"), "{}", r.id);
        }
    }

    #[test]
    fn dictation_has_one_key_per_option_under_flare_dictation_and_only_gpu_has_none() {
        assert_eq!(DICTATION_OPTIONS.len(), 10);
        let mut keys: Vec<&str> = Vec::new();
        for r in DICTATION_OPTIONS.iter() {
            if r.id == "dictation-gpu" {
                assert!(r.keys.is_empty());
                continue;
            }
            assert_eq!(r.keys.len(), 1, "{}", r.id);
            assert!(r.keys[0].starts_with("flare.dictation."), "{}", r.id);
            keys.push(r.keys[0]);
        }
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }

    #[test]
    fn help_is_its_own_list_under_flare_help() {
        assert_eq!(HELP_OPTIONS.len(), 1);
        assert_eq!(HELP_OPTIONS[0].id, "help-enabled");
        assert_eq!(HELP_OPTIONS[0].keys, &["flare.help.enabled"]);
        assert!(HELP_OPTIONS[0].keys[0].starts_with("flare.help."));
    }

    #[test]
    fn only_the_gpu_row_needs_nvidia() {
        let nvidia: Vec<&str> = all_rows().filter(|r| r.only_nvidia).map(|r| r.id).collect();
        assert_eq!(nvidia, vec!["dictation-gpu"]);
    }

    #[test]
    fn the_storage_keys_are_the_snapshot() {
        let core: Vec<String> = table_order()
            .into_iter()
            .filter(|r| !is_app(r.id))
            .map(store_line)
            .collect();
        assert_eq!(
            core,
            vec![
                "term-shell=flare.term.shell",
                "term-font-family=flare.term.font",
                "term-font=flare.term.fontPct",
                "agent-indicator=flare.term.agentIndicator",
                "agent-done-on=flare.term.agentDoneOn",
                "agent-rainbow=flare.term.agentRainbow",
                "agent-question-on=flare.term.agentQuestionOn",
                "agent-failed-on=flare.term.agentFailedOn",
                "agent-hooks=flare.term.agentHooks",
                "term-theme=flare.term.theme+flare.term.custom",
                "term-acrylic=flare.term.seeThrough",
                "agent-color=flare.term.agentColor",
                "agent-done-color=flare.term.agentDoneColor",
                "agent-question-color=flare.term.agentQuestionColor",
                "dictation-enabled=flare.dictation.enabled",
                "dictation-mode=flare.dictation.mode",
                "dictation-hotkey=flare.dictation.hotkey",
                "dictation-mic=flare.dictation.mic",
                "dictation-language=flare.dictation.language",
                "dictation-pause-media=flare.dictation.pauseMedia",
                "dictation-sounds=flare.dictation.sounds",
                "dictation-model=flare.dictation.model",
                "dictation-gpu=",
                "dictation-anywhere=flare.dictation.anywhere",
                "help-enabled=flare.help.enabled",
                "diag-verbose=",
                "diag-folder=",
                "diag-mark=",
            ]
        );
        assert_eq!(core.len(), 28);

        let app: Vec<String> = APP_OPTIONS.iter().map(store_line).collect();
        assert_eq!(
            app,
            vec![
                "tab-layout=flare.window.tabLayout+flare.window.sidebar+flare.window.sidebarWidth",
                "tab-switch=flare.window.tabSwitch",
                "window-edges=flare.window.edges",
                "window-background=flare.window.background",
                "window-secondary=flare.window.secondary",
                "window-accent=flare.window.accent",
                "newtab-mode=flare.newtab.mode+flare.newtab.folder",
                "explorer-verb=registry",
                "agent-keys=flare.window.agentKeys",
                "agent-guard=flare.window.agentGuard",
                "right-click-reports=flare.window.rightClickReports",
                "open-previews=flare.window.openPreviews",
                "wrap-previews=flare.window.wrapPreviews",
                "agent-eval=flare.web.agentEval",
                "app-network=flare.web.appNetwork",
                "dev-preview=flare.window.devPreview",
                "taskbar-badge=flare.window.taskbarBadge",
                "notify-ask=flare.window.notifyAsk",
                "app-version=",
            ]
        );
        assert_eq!(app.len(), 19);
    }

    #[test]
    fn every_row_has_a_known_section_a_known_icon_and_a_subtext() {
        for r in all_rows() {
            assert!(
                section_title(r.section).is_some(),
                "{} section {}",
                r.id,
                r.section
            );
            assert!(is_setting_icon(r.icon), "{} icon {}", r.id, r.icon);
            assert!(!r.label.is_empty(), "{} has no label", r.id);
            assert!(!r.sub.is_empty(), "{} has no subtext", r.id);
            assert!(
                PAGES.iter().any(|p| p.id == r.page),
                "{} page {}",
                r.id,
                r.page
            );
        }
    }

    #[test]
    fn the_order_inside_each_section_follows_the_tables() {
        let sections: BTreeSet<&str> = all_rows().map(|r| r.section).collect();
        for s in sections {
            let by_row_order: Vec<&str> = all_rows()
                .filter(|r| r.section == s)
                .map(|r| r.id)
                .collect();
            let by_table: Vec<&str> = table_order()
                .into_iter()
                .filter(|r| r.section == s)
                .map(|r| r.id)
                .collect();
            assert_eq!(by_row_order, by_table, "section {s}");
        }
    }

    #[test]
    fn labels_and_subs_follow_the_copy_rules() {
        for r in all_rows() {
            assert!(
                label_problem(r.label).is_none(),
                "{}: {:?}",
                r.id,
                label_problem(r.label)
            );
            assert!(
                copy_problem(r.sub).is_none(),
                "{}: {:?}",
                r.id,
                copy_problem(r.sub)
            );
            if r.id == "term-acrylic" {
                // The one row over the word limit, on purpose (O8, decision D5).
                assert!(sub_too_long(r.sub));
            } else {
                assert!(
                    !sub_too_long(r.sub),
                    "{} subtext runs past eight words",
                    r.id
                );
            }
        }
    }

    #[test]
    fn app_ids_are_unique_and_none_is_a_core_row() {
        let app_ids: Vec<&str> = APP_OPTIONS.iter().map(|a| a.id).collect();
        let mut sorted = app_ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), app_ids.len());
        let core_ids: Vec<&str> = TERMINAL_OPTIONS
            .iter()
            .chain(MARK_OPTIONS.iter())
            .chain(DICTATION_OPTIONS.iter())
            .chain(HELP_OPTIONS.iter())
            .chain(DIAGNOSTICS_OPTIONS.iter())
            .map(|r| r.id)
            .collect();
        for id in app_ids {
            assert!(!core_ids.contains(&id), "{id} is in a core list");
        }
    }

    #[test]
    fn app_rows_name_a_known_page_and_section() {
        for r in APP_OPTIONS.iter() {
            assert!(
                PAGES.iter().any(|p| p.id == r.page),
                "{} page {}",
                r.id,
                r.page
            );
            assert!(
                [
                    "window", "opening", "keys", "links", "theme", "marks", "notify", "about"
                ]
                .contains(&r.section),
                "{} section {}",
                r.id,
                r.section
            );
        }
    }

    #[test]
    fn every_row_key_is_stored_by_a_module() {
        // The dictation keys are read by flare-dictation's prefs module (M8 Task 7).
        let sources = [
            include_str!("../../flare-dictation/src/prefs.rs"),
            include_str!("help_prefs.rs"),
            include_str!("term_prefs.rs"),
            include_str!("tab_layout.rs"),
            include_str!("tab_switch.rs"),
            include_str!("sidebar.rs"),
            include_str!("new_tab.rs"),
            include_str!("agent_prefs.rs"),
            include_str!("look.rs"),
            include_str!("custom_theme.rs"),
            include_str!("window_colours.rs"),
        ];
        for r in all_rows() {
            for k in r.keys {
                let quoted = format!("\"{k}\"");
                assert!(
                    sources.iter().any(|s| s.contains(&quoted)),
                    "{} stores {k} in no module",
                    r.id
                );
            }
        }
    }

    #[test]
    fn row_finds_by_id_and_the_see_through_label_is_plain_copy() {
        let acrylic = row("term-acrylic").expect("term-acrylic is a row");
        assert_eq!(acrylic.label, "See through window");
        assert_eq!(acrylic.kind, RowKind::Range);
        assert_eq!(
            row("explorer-verb").map(|r| r.kind),
            Some(RowKind::Registry)
        );
        assert!(row("nope").is_none());
    }
}
