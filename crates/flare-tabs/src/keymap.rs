//! ChordId, CHORDS, chord_of, app_owns_key, key_label: the chord table (Task 10).
//! Port of Prism Terminal's keymap.ts: one table for the app's keys, read by the window filter
//! and by the terminal's "owns this key" check, so a chord never reaches the pty while the app
//! acts on it. Pure: what the chords depend on comes in through `KeyContext`.
//!
//! One table for both systems (macOS port phase 3): `KeyContext::os` says whose keys apply. On
//! Windows the app's chords are Ctrl chords; on macOS Cmd takes Ctrl's place (Cmd+T, Cmd+W, Cmd+,),
//! so every Ctrl+letter reaches the shell as a control character there. What stays Ctrl on both:
//! Ctrl+Tab (Mac apps step tabs with it too) and the agent guard's Ctrl+C and Ctrl+D, which guard
//! the shell's own keys. macOS adds Cmd+Shift+[ and ] for the tabs, Ctrl+Cmd+F for full screen and
//! the zoom keys (`MAC_ONLY`); Alt+Space (the Windows system menu) is Windows only.

use flare_term::keys::KeyEvent;
pub use flare_term::keys::KeyOs;

/// The app's chords, in the order the table is searched (PT's CHORDS order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordId {
    Help,
    Fullscreen,
    NextTab,
    JumpTab,
    Settings,
    FindTab,
    Sidebar,
    NewTab,
    CloseTab,
    Find,
    AgentGuard,
    /// Alt+Space: the system menu of the frameless window.
    SystemMenu,
    /// Ctrl+Shift+P: shows or hides the tab's panel (Surfaces stage 1).
    Panel,
    /// Ctrl+Shift+O: moves the keys between the terminal and the panel.
    PanelFocus,
    /// macOS Cmd+plus: the tab in front one size up (Windows zooms with Ctrl+wheel only).
    ZoomIn,
    /// macOS Cmd+minus: one size down.
    ZoomOut,
    /// macOS Cmd+0: back to the size the settings give.
    ZoomReset,
}

/// Every chord, in search order.
pub const ORDER: [ChordId; 14] = [
    ChordId::Help,
    ChordId::Fullscreen,
    ChordId::NextTab,
    ChordId::JumpTab,
    ChordId::Settings,
    ChordId::FindTab,
    ChordId::Sidebar,
    ChordId::NewTab,
    ChordId::CloseTab,
    ChordId::Find,
    ChordId::AgentGuard,
    ChordId::SystemMenu,
    ChordId::Panel,
    ChordId::PanelFocus,
];

/// The chords only macOS has, searched after `ORDER` there.
pub const MAC_ONLY: [ChordId; 3] = [ChordId::ZoomIn, ChordId::ZoomOut, ChordId::ZoomReset];

/// What the chords depend on, handed in by the window (never read here).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyContext {
    /// Whose keys: Windows (the default) or macOS.
    pub os: KeyOs,
    /// Command help is switched on (Settings).
    pub help: bool,
    /// The tabs are a sidebar.
    pub vertical: bool,
    /// The terminal in front runs a full-screen program (vim, less).
    pub full_screen: bool,
    /// Agent hotkeys are on: a chord's agent key is the app's too.
    pub agent_keys: bool,
    /// Keep agent sessions open is on.
    pub agent_guard: bool,
    /// An agent runs in the tab in front.
    pub agent_here: bool,
    /// That agent is working.
    pub agent_working: bool,
    /// That agent holds a selection of its own (its fullscreen view).
    pub agent_selecting: bool,
}

/// What a chord does under a question or the update window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnderModal {
    /// Does nothing, and keeps the key from the shell behind.
    Swallow,
    /// Acts as ever.
    Act,
}

/// A second key for the same action, the app's only while agent hotkeys are on.
pub struct AgentChord {
    pub label: &'static str,
    pub matches: fn(&KeyEvent, &KeyContext) -> bool,
}

/// One chord's definition.
pub struct ChordDef {
    /// What the UI shows on Windows ("" for a chord Windows does not have).
    pub label: &'static str,
    /// What the UI shows on macOS, in the Mac glyphs ("" for a chord the Mac does not have).
    pub mac_label: &'static str,
    /// The agent-hotkey variant's Mac label, when there is one.
    pub mac_agent_label: &'static str,
    /// The shell still hears it.
    pub shell_too: bool,
    /// Behaviour under a question.
    pub under_modal: UnderModal,
    /// Does this key press match the chord?
    pub matches: fn(&KeyEvent, &KeyContext) -> bool,
    /// The agent-hotkey variant, when there is one.
    pub agent: Option<AgentChord>,
}

fn bare(e: &KeyEvent) -> bool {
    !e.ctrl && !e.alt && !e.shift && !e.meta
}

fn ctrl(e: &KeyEvent) -> bool {
    e.ctrl && !e.alt && !e.meta
}

/// The app's modifier for this system: Ctrl on Windows, Cmd on macOS (Shift allowed).
fn primary(e: &KeyEvent, c: &KeyContext) -> bool {
    c.os.primary(e)
}

/// Cmd alone on macOS (never true on Windows).
fn mac_cmd(e: &KeyEvent, c: &KeyContext) -> bool {
    c.os.is_mac() && e.meta && !e.ctrl && !e.alt
}

/// macOS Cmd+Shift+[ or ]: -1 for the tab before, 1 for the one after; None for any other key.
/// The bracket by the typed key ("[" or "{" with Shift) or, for a layout that types something
/// else there, the physical key.
pub fn bracket_step(e: &KeyEvent, c: &KeyContext) -> Option<i32> {
    if !mac_cmd(e, c) || !e.shift {
        return None;
    }
    match (e.key.as_str(), e.code.as_str()) {
        ("[" | "{", _) | (_, "BracketLeft") => Some(-1),
        ("]" | "}", _) | (_, "BracketRight") => Some(1),
        _ => None,
    }
}

fn k(e: &KeyEvent) -> String {
    e.key.to_lowercase()
}

/// The letter, by the key the layout types; the physical key answers only for a key named outside
/// ASCII (Russian, Greek), as in flare-term's `is_key` (regression rule 11).
fn letter(e: &KeyEvent, l: char) -> bool {
    let mut chars = e.key.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii()
    {
        return c.to_ascii_lowercase() == l;
    }
    e.code == format!("Key{}", l.to_ascii_uppercase())
}

fn is_digit_1_to_9(e: &KeyEvent) -> bool {
    e.key.len() == 1 && matches!(e.key.as_bytes()[0], b'1'..=b'9')
}

fn help_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    e.key == "F1" && bare(e) && c.help
}

fn fullscreen_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    e.key == "F11" || (c.os.is_mac() && e.ctrl && e.meta && !e.alt && !e.shift && letter(e, 'f'))
}

fn next_tab_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    (ctrl(e) && e.key == "Tab") || bracket_step(e, c).is_some()
}

fn next_tab_agent_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && !e.shift && (e.key == "PageDown" || e.key == "PageUp")
}

fn jump_tab_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && !e.shift && is_digit_1_to_9(e)
}

fn settings_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && !e.shift && e.key == ","
}

fn find_tab_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && e.shift && k(e) == "k"
}

fn sidebar_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && e.shift && k(e) == "b" && c.vertical
}

fn sidebar_agent_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && !e.shift && k(e) == "s" && c.vertical
}

fn new_tab_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && k(e) == "t"
}

fn close_tab_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && k(e) == "w"
}

/// Windows: a full-screen program keeps Ctrl+F (Ctrl+Shift+F still finds). macOS: a program never
/// hears a Cmd key, so Cmd+F always finds.
fn find_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && k(e) == "f" && (e.shift || !c.full_screen || c.os.is_mac())
}

fn agent_guard_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    c.agent_guard
        && c.agent_here
        && ctrl(e)
        && !e.shift
        && (letter(e, 'd') || (letter(e, 'c') && !c.agent_working && !c.agent_selecting))
}

fn system_menu_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    !c.os.is_mac()
        && e.alt
        && !e.ctrl
        && !e.shift
        && !e.meta
        && (e.key == " " || e.key == "Space" || e.code == "Space")
}

fn panel_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && e.shift && letter(e, 'p')
}

fn panel_focus_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    primary(e, c) && e.shift && letter(e, 'o')
}

/// Cmd+= or Cmd+plus (the same key, with or without Shift).
fn zoom_in_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    mac_cmd(e, c)
        && (matches!(e.key.as_str(), "=" | "+") || e.code == "Equal" || e.code == "NumpadAdd")
}

fn zoom_out_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    mac_cmd(e, c) && !e.shift && (e.key == "-" || e.code == "Minus" || e.code == "NumpadSubtract")
}

fn zoom_reset_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    mac_cmd(e, c) && !e.shift && (e.key == "0" || e.code == "Digit0" || e.code == "Numpad0")
}

static HELP: ChordDef = ChordDef {
    label: "F1",
    mac_label: "F1",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: help_matches,
    agent: None,
};

static FULLSCREEN: ChordDef = ChordDef {
    label: "F11",
    mac_label: "\u{2303}\u{2318}F",
    mac_agent_label: "",
    shell_too: true,
    under_modal: UnderModal::Act,
    matches: fullscreen_matches,
    agent: None,
};

static NEXT_TAB: ChordDef = ChordDef {
    label: "Ctrl+Tab",
    mac_label: "\u{2303}Tab",
    mac_agent_label: "\u{2303}PgDn",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: next_tab_matches,
    agent: Some(AgentChord {
        label: "Ctrl+PgDn",
        matches: next_tab_agent_matches,
    }),
};

static JUMP_TAB: ChordDef = ChordDef {
    label: "Ctrl+1..9",
    mac_label: "\u{2318}1..9",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: jump_tab_matches,
    agent: None,
};

static SETTINGS: ChordDef = ChordDef {
    label: "Ctrl+,",
    mac_label: "\u{2318},",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: settings_matches,
    agent: None,
};

static FIND_TAB: ChordDef = ChordDef {
    label: "Ctrl+Shift+K",
    mac_label: "\u{21e7}\u{2318}K",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: find_tab_matches,
    agent: None,
};

static SIDEBAR: ChordDef = ChordDef {
    label: "Ctrl+Shift+B",
    mac_label: "\u{21e7}\u{2318}B",
    mac_agent_label: "\u{2318}S",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: sidebar_matches,
    agent: Some(AgentChord {
        label: "Ctrl+S",
        matches: sidebar_agent_matches,
    }),
};

static NEW_TAB: ChordDef = ChordDef {
    label: "Ctrl+T",
    mac_label: "\u{2318}T",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: new_tab_matches,
    agent: None,
};

static CLOSE_TAB: ChordDef = ChordDef {
    label: "Ctrl+W",
    mac_label: "\u{2318}W",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: close_tab_matches,
    agent: None,
};

static FIND: ChordDef = ChordDef {
    label: "Ctrl+F",
    mac_label: "\u{2318}F",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: find_matches,
    agent: None,
};

static AGENT_GUARD: ChordDef = ChordDef {
    label: "Ctrl+D",
    mac_label: "\u{2303}D",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: agent_guard_matches,
    agent: None,
};

static SYSTEM_MENU: ChordDef = ChordDef {
    label: "Alt+Space",
    mac_label: "",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: system_menu_matches,
    agent: None,
};

static PANEL: ChordDef = ChordDef {
    label: "Ctrl+Shift+P",
    mac_label: "\u{21e7}\u{2318}P",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: panel_matches,
    agent: None,
};

static PANEL_FOCUS: ChordDef = ChordDef {
    label: "Ctrl+Shift+O",
    mac_label: "\u{21e7}\u{2318}O",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: panel_focus_matches,
    agent: None,
};

static ZOOM_IN: ChordDef = ChordDef {
    label: "",
    mac_label: "\u{2318}+",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: zoom_in_matches,
    agent: None,
};

static ZOOM_OUT: ChordDef = ChordDef {
    label: "",
    mac_label: "\u{2318}-",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: zoom_out_matches,
    agent: None,
};

static ZOOM_RESET: ChordDef = ChordDef {
    label: "",
    mac_label: "\u{2318}0",
    mac_agent_label: "",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: zoom_reset_matches,
    agent: None,
};

/// The definition of one chord.
pub fn chord(id: ChordId) -> &'static ChordDef {
    match id {
        ChordId::Help => &HELP,
        ChordId::Fullscreen => &FULLSCREEN,
        ChordId::NextTab => &NEXT_TAB,
        ChordId::JumpTab => &JUMP_TAB,
        ChordId::Settings => &SETTINGS,
        ChordId::FindTab => &FIND_TAB,
        ChordId::Sidebar => &SIDEBAR,
        ChordId::NewTab => &NEW_TAB,
        ChordId::CloseTab => &CLOSE_TAB,
        ChordId::Find => &FIND,
        ChordId::AgentGuard => &AGENT_GUARD,
        ChordId::SystemMenu => &SYSTEM_MENU,
        ChordId::Panel => &PANEL,
        ChordId::PanelFocus => &PANEL_FOCUS,
        ChordId::ZoomIn => &ZOOM_IN,
        ChordId::ZoomOut => &ZOOM_OUT,
        ChordId::ZoomReset => &ZOOM_RESET,
    }
}

/// The app's chord for this key press, or None when it is the shell's. A key UP is never a chord.
/// The first id in `ORDER` whose `matches` holds; else, while agent keys are on, the first id whose
/// agent key matches.
pub fn chord_of(e: &KeyEvent, c: &KeyContext) -> Option<ChordId> {
    if !e.pressed {
        return None;
    }
    if let Some(id) = ORDER.into_iter().find(|id| (chord(*id).matches)(e, c)) {
        return Some(id);
    }
    if c.os.is_mac()
        && let Some(id) = MAC_ONLY.into_iter().find(|id| (chord(*id).matches)(e, c))
    {
        return Some(id);
    }
    if !c.agent_keys {
        return None;
    }
    ORDER
        .into_iter()
        .find(|id| chord(*id).agent.as_ref().is_some_and(|a| (a.matches)(e, c)))
}

/// Does the app take this key from the shell? The shell hears only the chords marked shell_too.
pub fn app_owns_key(e: &KeyEvent, c: &KeyContext) -> bool {
    match chord_of(e, c) {
        Some(id) => !chord(id).shell_too,
        None => false,
    }
}

/// What the UI calls a chord on Windows: its agent key where agent keys are on and it has one.
pub fn key_label(id: ChordId, agent_keys: bool) -> &'static str {
    key_label_on(KeyOs::Windows, id, agent_keys)
}

/// What the UI calls a chord on this system ("" for a chord the system does not have).
pub fn key_label_on(os: KeyOs, id: ChordId, agent_keys: bool) -> &'static str {
    let def = chord(id);
    match (os, &def.agent, agent_keys) {
        (KeyOs::Windows, Some(a), true) => a.label,
        (KeyOs::Windows, _, _) => def.label,
        (KeyOs::Mac, Some(_), true) => def.mac_agent_label,
        (KeyOs::Mac, _, _) => def.mac_label,
    }
}

/// The copy and paste keys the terminal menu shows (they live in the terminal's rules, not here).
pub fn copy_paste_labels(os: KeyOs) -> (&'static str, &'static str) {
    match os {
        KeyOs::Windows => ("Ctrl+C", "Ctrl+V"),
        KeyOs::Mac => ("\u{2318}C", "\u{2318}V"),
    }
}

/// The chords that agent hotkeys add (Next tab and Sidebar).
pub fn agent_chords() -> Vec<ChordId> {
    ORDER
        .into_iter()
        .filter(|id| chord(*id).agent.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> KeyContext {
        KeyContext {
            os: KeyOs::Windows,
            help: true,
            vertical: true,
            full_screen: false,
            agent_keys: false,
            agent_guard: true,
            agent_here: true,
            agent_working: false,
            agent_selecting: false,
        }
    }

    fn ev(key: &str) -> KeyEvent {
        KeyEvent::new(key, "")
    }

    /// Parses a label such as "Ctrl+Shift+K" into the key press it stands for.
    fn from_label(label: &str) -> KeyEvent {
        let parts: Vec<&str> = label.split('+').collect();
        let (key, mods) = parts.split_last().expect("a label has a key");
        let mut e = KeyEvent::new(&key.replace("1..9", "1"), "");
        for m in mods {
            match *m {
                "Ctrl" => e.ctrl = true,
                "Shift" => e.shift = true,
                "Alt" => e.alt = true,
                other => panic!("unknown modifier {other}"),
            }
        }
        e
    }

    #[test]
    fn the_agent_guard_reads_the_layout_letter_first() {
        let c = on();
        // Dvorak: Ctrl+J sits on the physical C key, Ctrl+E on D: both are Claude's, not the guard's.
        assert_ne!(
            chord_of(&KeyEvent::new("j", "KeyC").ctrl(), &c),
            Some(ChordId::AgentGuard)
        );
        assert_ne!(
            chord_of(&KeyEvent::new("e", "KeyD").ctrl(), &c),
            Some(ChordId::AgentGuard)
        );
        // Dvorak's real Ctrl+C and Ctrl+D (physical J and E) are still the guard's.
        assert_eq!(
            chord_of(&KeyEvent::new("c", "KeyJ").ctrl(), &c),
            Some(ChordId::AgentGuard)
        );
        assert_eq!(
            chord_of(&KeyEvent::new("d", "KeyE").ctrl(), &c),
            Some(ChordId::AgentGuard)
        );
        // A key named outside ASCII (Russian) falls back to the physical key.
        assert_eq!(
            chord_of(&KeyEvent::new("\u{441}", "KeyC").ctrl(), &c),
            Some(ChordId::AgentGuard)
        );
    }

    #[test]
    fn agent_guard_takes_ctrl_d_always_and_ctrl_c_while_idle() {
        let c = on();
        assert_eq!(chord_of(&ev("d").ctrl(), &c), Some(ChordId::AgentGuard));
        assert_eq!(chord_of(&ev("c").ctrl(), &c), Some(ChordId::AgentGuard));
        let working = KeyContext {
            agent_working: true,
            ..c
        };
        assert_eq!(
            chord_of(&ev("d").ctrl(), &working),
            Some(ChordId::AgentGuard)
        );
    }

    #[test]
    fn agent_guard_leaves_ctrl_c_to_an_agent_at_work() {
        let working = KeyContext {
            agent_working: true,
            ..on()
        };
        assert_eq!(chord_of(&ev("c").ctrl(), &working), None);
    }

    #[test]
    fn agent_guard_leaves_ctrl_c_to_an_agent_with_a_selection() {
        let selecting = KeyContext {
            agent_selecting: true,
            ..on()
        };
        assert_eq!(chord_of(&ev("c").ctrl(), &selecting), None);
        assert_eq!(
            chord_of(&ev("d").ctrl(), &selecting),
            Some(ChordId::AgentGuard)
        );
    }

    #[test]
    fn agent_guard_is_off_in_a_plain_shell_and_when_the_setting_is_off() {
        let plain = KeyContext {
            agent_here: false,
            ..on()
        };
        assert_eq!(chord_of(&ev("c").ctrl(), &plain), None);
        assert_eq!(chord_of(&ev("d").ctrl(), &plain), None);
        let off = KeyContext {
            agent_guard: false,
            ..on()
        };
        assert_eq!(chord_of(&ev("c").ctrl(), &off), None);
        assert_eq!(chord_of(&ev("d").ctrl(), &off), None);
    }

    #[test]
    fn agent_guard_matches_the_physical_key_and_leaves_ctrl_shift_c_alone() {
        assert_eq!(
            chord_of(&KeyEvent::new("\u{441}", "KeyC").ctrl(), &on()),
            Some(ChordId::AgentGuard)
        );
        assert_eq!(
            chord_of(&KeyEvent::new("C", "").ctrl().shift(), &on()),
            None
        );
    }

    #[test]
    fn alt_space_is_a_host_chord_and_never_reaches_the_pty() {
        let c = on();
        for e in [
            KeyEvent::new(" ", "Space").alt(),
            KeyEvent::new("Space", "").alt(),
        ] {
            assert_eq!(chord_of(&e, &c), Some(ChordId::SystemMenu));
            assert!(app_owns_key(&e, &c));
        }
        assert_eq!(chord_of(&KeyEvent::new(" ", "Space"), &c), None);
        assert_eq!(
            chord_of(&KeyEvent::new(" ", "Space").alt().ctrl(), &c),
            None
        );
    }

    #[test]
    fn the_panel_chords_are_host_chords_and_plain_ctrl_p_and_o_stay_the_shells() {
        let c = on();
        let p = KeyEvent::new("P", "KeyP").ctrl().shift();
        let o = KeyEvent::new("o", "KeyO").ctrl().shift();
        assert_eq!(chord_of(&p, &c), Some(ChordId::Panel));
        assert_eq!(chord_of(&o, &c), Some(ChordId::PanelFocus));
        assert!(app_owns_key(&p, &c) && app_owns_key(&o, &c));
        // The physical key counts when a layout moves the letter (rule 11).
        assert_eq!(
            chord_of(&KeyEvent::new("\u{437}", "KeyP").ctrl().shift(), &c),
            Some(ChordId::Panel)
        );
        for plain in [
            KeyEvent::new("p", "KeyP").ctrl(),
            KeyEvent::new("o", "KeyO").ctrl(),
        ] {
            assert_eq!(chord_of(&plain, &c), None);
            assert!(!app_owns_key(&plain, &c));
        }
        // With Alt too (AltGr on some layouts) the key is the shell's.
        assert_eq!(
            chord_of(&KeyEvent::new("P", "KeyP").ctrl().shift().alt(), &c),
            None
        );
    }

    #[test]
    fn ctrl_shift_a_is_the_shells_again_since_the_agent_board_was_removed() {
        // The agent board (and its Ctrl+Shift+A) was removed on 2026-10-22; the key reaches the shell.
        let c = on();
        for k in [
            KeyEvent::new("A", "KeyA").ctrl().shift(),
            KeyEvent::new("a", "KeyA").ctrl(),
        ] {
            assert_eq!(chord_of(&k, &c), None);
            assert!(!app_owns_key(&k, &c));
        }
        assert!(
            ORDER.iter().all(|id| chord(*id).label != "Ctrl+Shift+A"),
            "no chord answers to Ctrl+Shift+A"
        );
    }

    #[test]
    fn every_label_is_the_key_that_chord_answers_to() {
        let c = on();
        for id in ORDER {
            let label = key_label(id, false);
            assert_eq!(chord_of(&from_label(label), &c), Some(id), "label {label}");
        }
    }

    #[test]
    fn the_app_takes_exactly_the_chords_it_acts_on() {
        let c = on();
        for id in ORDER {
            let label = key_label(id, false);
            assert_eq!(
                app_owns_key(&from_label(label), &c),
                id != ChordId::Fullscreen,
                "label {label}"
            );
        }
    }

    #[test]
    fn agent_hotkeys_add_second_keys() {
        let agent = KeyContext {
            agent_keys: true,
            ..on()
        };
        let ctrl_s = ev("s").ctrl();
        assert_eq!(chord_of(&ctrl_s, &agent), Some(ChordId::Sidebar));
        assert!(app_owns_key(&ctrl_s, &agent));
        let no_vertical = KeyContext {
            vertical: false,
            ..agent
        };
        assert_eq!(chord_of(&ctrl_s, &no_vertical), None);
        assert_eq!(
            chord_of(&KeyEvent::new("PageDown", "").ctrl(), &agent),
            Some(ChordId::NextTab)
        );
        assert_eq!(
            chord_of(&KeyEvent::new("PageUp", "").ctrl(), &agent),
            Some(ChordId::NextTab)
        );
        assert_eq!(
            chord_of(&KeyEvent::new("K", "").ctrl().shift(), &agent),
            Some(ChordId::FindTab)
        );
        assert_eq!(
            chord_of(&from_label("Ctrl+Shift+B"), &agent),
            Some(ChordId::Sidebar)
        );
        assert_eq!(
            chord_of(&from_label("Ctrl+Tab"), &agent),
            Some(ChordId::NextTab)
        );
        // Off: the three keys are the shell's again.
        let off = on();
        assert_eq!(chord_of(&ctrl_s, &off), None);
        assert!(!app_owns_key(&ctrl_s, &off));
        assert_eq!(chord_of(&KeyEvent::new("PageDown", "").ctrl(), &off), None);
        assert_eq!(chord_of(&KeyEvent::new("PageUp", "").ctrl(), &off), None);
        // Claude Code's own keys with Ctrl and no Shift are not ours.
        for key in ["b", "o", "r", "g", "k", "l", "j"] {
            assert_eq!(chord_of(&ev(key).ctrl(), &agent), None, "ctrl+{key}");
        }
        assert_eq!(key_label(ChordId::Sidebar, true), "Ctrl+S");
        assert_eq!(key_label(ChordId::Sidebar, false), "Ctrl+Shift+B");
    }

    #[test]
    fn the_shell_keeps_its_own_keys() {
        let plain = KeyContext {
            agent_here: false,
            ..on()
        };
        for key in ["b", "s", "k", "c", "v"] {
            assert_eq!(chord_of(&ev(key).ctrl(), &plain), None, "ctrl+{key}");
        }
        assert_eq!(
            chord_of(&KeyEvent::new("B", "").ctrl().shift().alt(), &plain),
            None
        );
        assert_eq!(chord_of(&KeyEvent::new("F1", "").shift(), &plain), None);
    }

    #[test]
    fn sidebar_key_only_while_vertical_find_a_tab_in_both() {
        let top = KeyContext {
            vertical: false,
            ..on()
        };
        assert_eq!(chord_of(&KeyEvent::new("B", "").ctrl().shift(), &top), None);
        assert_eq!(
            chord_of(&KeyEvent::new("K", "").ctrl().shift(), &top),
            Some(ChordId::FindTab)
        );
        assert_eq!(
            chord_of(&KeyEvent::new("b", "").ctrl().shift(), &on()),
            Some(ChordId::Sidebar)
        );
    }

    #[test]
    fn f1_and_find_rules() {
        let no_help = KeyContext {
            help: false,
            ..on()
        };
        assert_eq!(chord_of(&ev("F1"), &no_help), None);
        let full = KeyContext {
            full_screen: true,
            ..on()
        };
        assert_eq!(chord_of(&ev("f").ctrl(), &full), None);
        assert_eq!(
            chord_of(&KeyEvent::new("F", "").ctrl().shift(), &full),
            Some(ChordId::Find)
        );
    }

    #[test]
    fn no_slint_file_types_a_shortcut_label_by_hand() {
        let mut labels: Vec<&str> = Vec::new();
        for id in ORDER {
            for agent_keys in [false, true] {
                labels.push(key_label(id, agent_keys));
            }
        }
        let banned: Vec<&str> = labels
            .into_iter()
            .filter(|l| l.contains('+') || *l == "F1" || *l == "F11")
            .collect();

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../flare/ui");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("flare/ui exists") {
            let path = entry.expect("a dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("slint") {
                continue;
            }
            checked += 1;
            // sync-ok: a test reads the Slint sources once
            let text = std::fs::read_to_string(&path).expect("a slint file reads");
            for line in text.lines() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for (i, quoted) in line.split('"').enumerate() {
                    // Odd pieces sit between a pair of quotes.
                    if i % 2 == 1 && banned.contains(&quoted) {
                        panic!("{} types the label {quoted:?}: {line}", path.display());
                    }
                }
            }
        }
        assert!(checked > 0, "no .slint files found");
    }

    #[test]
    fn a_key_up_is_never_a_chord() {
        assert_eq!(chord_of(&ev("t").ctrl().released(), &on()), None);
    }

    #[test]
    fn ctrl_w_and_ctrl_shift_w_both_close_ctrl_t_and_ctrl_shift_t_both_open() {
        let c = on();
        assert_eq!(chord_of(&ev("w").ctrl(), &c), Some(ChordId::CloseTab));
        assert_eq!(
            chord_of(&KeyEvent::new("W", "").ctrl().shift(), &c),
            Some(ChordId::CloseTab)
        );
        assert_eq!(chord_of(&ev("t").ctrl(), &c), Some(ChordId::NewTab));
        assert_eq!(
            chord_of(&KeyEvent::new("T", "").ctrl().shift(), &c),
            Some(ChordId::NewTab)
        );
    }

    #[test]
    fn jump_tab_takes_1_to_9_only_without_shift() {
        let c = on();
        for d in 1..=9 {
            let key = d.to_string();
            assert_eq!(
                chord_of(&KeyEvent::new(&key, "").ctrl(), &c),
                Some(ChordId::JumpTab),
                "ctrl+{key}"
            );
        }
        assert_eq!(chord_of(&ev("0").ctrl(), &c), None);
        assert_eq!(chord_of(&ev("1").ctrl().shift(), &c), None);
    }

    #[test]
    fn under_a_question_these_are_swallowed_and_these_act() {
        for id in ORDER {
            let expected = match id {
                ChordId::Help | ChordId::Fullscreen | ChordId::CloseTab | ChordId::SystemMenu => {
                    UnderModal::Act
                }
                _ => UnderModal::Swallow,
            };
            assert_eq!(chord(id).under_modal, expected, "{id:?}");
        }
    }

    #[test]
    fn agent_chords_are_next_tab_and_sidebar() {
        assert_eq!(agent_chords(), vec![ChordId::NextTab, ChordId::Sidebar]);
    }

    // ---- macOS (port phase 3) ---------------------------------------------------------------------

    fn mac() -> KeyContext {
        KeyContext {
            os: KeyOs::Mac,
            ..on()
        }
    }

    /// Parses a Mac label such as "\u{21e7}\u{2318}K" into the key press it stands for.
    fn from_mac_label(label: &str) -> KeyEvent {
        let mut e = KeyEvent::new("", "");
        let mut rest = String::new();
        for c in label.chars() {
            match c {
                '\u{2303}' => e.ctrl = true,
                '\u{2325}' => e.alt = true,
                '\u{21e7}' => e.shift = true,
                '\u{2318}' => e.meta = true,
                other => rest.push(other),
            }
        }
        let key = rest.replace("1..9", "1").replace("PgDn", "PageDown");
        let key = if e.shift || key.chars().count() != 1 {
            key
        } else {
            key.to_lowercase()
        };
        e.key = key;
        e
    }

    #[test]
    fn every_mac_label_is_the_key_that_chord_answers_to() {
        let c = mac();
        for id in ORDER.into_iter().chain(MAC_ONLY) {
            let label = key_label_on(KeyOs::Mac, id, false);
            if id == ChordId::SystemMenu {
                assert_eq!(label, "", "no system menu on a Mac");
                continue;
            }
            assert_eq!(
                chord_of(&from_mac_label(label), &c),
                Some(id),
                "label {label}"
            );
        }
        let agent = KeyContext {
            agent_keys: true,
            ..c
        };
        for id in agent_chords() {
            let label = key_label_on(KeyOs::Mac, id, true);
            assert_eq!(
                chord_of(&from_mac_label(label), &agent),
                Some(id),
                "agent label {label}"
            );
        }
    }

    #[test]
    fn on_the_mac_the_app_takes_exactly_the_chords_it_acts_on() {
        let c = mac();
        for id in ORDER.into_iter().chain(MAC_ONLY) {
            if id == ChordId::SystemMenu {
                continue;
            }
            let label = key_label_on(KeyOs::Mac, id, false);
            assert_eq!(
                app_owns_key(&from_mac_label(label), &c),
                id != ChordId::Fullscreen,
                "label {label}"
            );
        }
    }

    #[test]
    fn on_the_mac_ctrl_letters_and_digits_are_never_app_chords() {
        let c = KeyContext {
            agent_here: false,
            ..mac()
        };
        for key in [
            "t", "w", "f", "k", "b", "p", "o", "s", "1", "9", ",", "a", "e", "c", "v", "d", "u",
        ] {
            assert_eq!(chord_of(&ev(key).ctrl(), &c), None, "ctrl+{key}");
            assert_eq!(
                chord_of(&ev(&key.to_uppercase()).ctrl().shift(), &c),
                None,
                "ctrl+shift+{key}"
            );
        }
    }

    #[test]
    fn on_windows_cmd_keys_are_never_chords() {
        let c = on();
        for key in ["t", "w", "f", ",", "1", "=", "-", "0"] {
            assert_eq!(chord_of(&ev(key).meta(), &c), None, "cmd+{key}");
        }
        assert_eq!(
            chord_of(&KeyEvent::new("}", "BracketRight").meta().shift(), &c),
            None
        );
        assert_eq!(chord_of(&ev("f").meta().ctrl(), &c), None);
        for id in MAC_ONLY {
            assert_eq!(key_label(id, false), "", "{id:?}");
        }
    }

    #[test]
    fn on_the_mac_cmd_f_finds_even_over_a_full_screen_program() {
        let full = KeyContext {
            full_screen: true,
            ..mac()
        };
        assert_eq!(chord_of(&ev("f").meta(), &full), Some(ChordId::Find));
        // Ctrl+F there is the program's (and the shell's on a Mac, always).
        assert_eq!(chord_of(&ev("f").ctrl(), &full), None);
    }

    #[test]
    fn on_the_mac_brackets_step_by_the_typed_or_the_physical_key() {
        let c = mac();
        assert_eq!(
            bracket_step(&KeyEvent::new("{", "").meta().shift(), &c),
            Some(-1)
        );
        assert_eq!(
            bracket_step(&KeyEvent::new("}", "").meta().shift(), &c),
            Some(1)
        );
        // A layout that types another character on the bracket keys.
        assert_eq!(
            bracket_step(&KeyEvent::new("\u{c5}", "BracketLeft").meta().shift(), &c),
            Some(-1)
        );
        assert_eq!(
            bracket_step(&KeyEvent::new("]", "BracketRight").meta(), &c),
            None
        );
        assert_eq!(
            bracket_step(&KeyEvent::new("}", "").ctrl().shift(), &on()),
            None
        );
    }

    #[test]
    fn on_the_mac_the_zoom_keys_take_plus_equals_minus_and_zero() {
        let c = mac();
        assert_eq!(chord_of(&ev("=").meta(), &c), Some(ChordId::ZoomIn));
        assert_eq!(chord_of(&ev("+").meta().shift(), &c), Some(ChordId::ZoomIn));
        assert_eq!(chord_of(&ev("+").meta(), &c), Some(ChordId::ZoomIn));
        assert_eq!(chord_of(&ev("-").meta(), &c), Some(ChordId::ZoomOut));
        assert_eq!(chord_of(&ev("0").meta(), &c), Some(ChordId::ZoomReset));
        assert_eq!(chord_of(&ev("0").ctrl(), &c), None);
    }

    #[test]
    fn copy_and_paste_labels_follow_the_system() {
        assert_eq!(copy_paste_labels(KeyOs::Windows), ("Ctrl+C", "Ctrl+V"));
        assert_eq!(copy_paste_labels(KeyOs::Mac), ("\u{2318}C", "\u{2318}V"));
        assert_eq!(
            key_label_on(KeyOs::Mac, ChordId::NewTab, false),
            "\u{2318}T"
        );
        assert_eq!(
            key_label_on(KeyOs::Mac, ChordId::Sidebar, true),
            "\u{2318}S"
        );
        assert_eq!(
            key_label_on(KeyOs::Windows, ChordId::Sidebar, true),
            "Ctrl+S"
        );
    }
}
