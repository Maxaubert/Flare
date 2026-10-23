//! ChordId, CHORDS, chord_of, app_owns_key, key_label: the chord table (Task 10).
//! Port of Prism Terminal's keymap.ts: one table for the app's keys, read by the window filter
//! and by the terminal's "owns this key" check, so a chord never reaches the pty while the app
//! acts on it. Pure: what the chords depend on comes in through `KeyContext`.

use flare_term::keys::KeyEvent;

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
    /// Ctrl+Shift+A: shows or hides the agent board in the tab's panel (Surfaces stage 7).
    Board,
}

/// Every chord, in search order.
pub const ORDER: [ChordId; 15] = [
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
    ChordId::Board,
];

/// What the chords depend on, handed in by the window (never read here).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyContext {
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
    /// What the UI shows.
    pub label: &'static str,
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

fn k(e: &KeyEvent) -> String {
    e.key.to_lowercase()
}

/// The letter, by the key or by the physical key (regression rule 11).
fn letter(e: &KeyEvent, l: char) -> bool {
    k(e) == l.to_string() || e.code == format!("Key{}", l.to_ascii_uppercase())
}

fn is_digit_1_to_9(e: &KeyEvent) -> bool {
    e.key.len() == 1 && matches!(e.key.as_bytes()[0], b'1'..=b'9')
}

fn help_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    e.key == "F1" && bare(e) && c.help
}

fn fullscreen_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    e.key == "F11"
}

fn next_tab_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && e.key == "Tab"
}

fn next_tab_agent_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && !e.shift && (e.key == "PageDown" || e.key == "PageUp")
}

fn jump_tab_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && !e.shift && is_digit_1_to_9(e)
}

fn settings_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && !e.shift && e.key == ","
}

fn find_tab_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && e.shift && k(e) == "k"
}

fn sidebar_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    ctrl(e) && e.shift && k(e) == "b" && c.vertical
}

fn sidebar_agent_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    ctrl(e) && !e.shift && k(e) == "s" && c.vertical
}

fn new_tab_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && k(e) == "t"
}

fn close_tab_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && k(e) == "w"
}

fn find_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    ctrl(e) && k(e) == "f" && (e.shift || !c.full_screen)
}

fn agent_guard_matches(e: &KeyEvent, c: &KeyContext) -> bool {
    c.agent_guard
        && c.agent_here
        && ctrl(e)
        && !e.shift
        && (letter(e, 'd') || (letter(e, 'c') && !c.agent_working && !c.agent_selecting))
}

fn system_menu_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    e.alt
        && !e.ctrl
        && !e.shift
        && !e.meta
        && (e.key == " " || e.key == "Space" || e.code == "Space")
}

fn panel_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && e.shift && letter(e, 'p')
}

fn panel_focus_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && e.shift && letter(e, 'o')
}

static HELP: ChordDef = ChordDef {
    label: "F1",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: help_matches,
    agent: None,
};

static FULLSCREEN: ChordDef = ChordDef {
    label: "F11",
    shell_too: true,
    under_modal: UnderModal::Act,
    matches: fullscreen_matches,
    agent: None,
};

static NEXT_TAB: ChordDef = ChordDef {
    label: "Ctrl+Tab",
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
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: jump_tab_matches,
    agent: None,
};

static SETTINGS: ChordDef = ChordDef {
    label: "Ctrl+,",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: settings_matches,
    agent: None,
};

static FIND_TAB: ChordDef = ChordDef {
    label: "Ctrl+Shift+K",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: find_tab_matches,
    agent: None,
};

static SIDEBAR: ChordDef = ChordDef {
    label: "Ctrl+Shift+B",
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
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: new_tab_matches,
    agent: None,
};

static CLOSE_TAB: ChordDef = ChordDef {
    label: "Ctrl+W",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: close_tab_matches,
    agent: None,
};

static FIND: ChordDef = ChordDef {
    label: "Ctrl+F",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: find_matches,
    agent: None,
};

static AGENT_GUARD: ChordDef = ChordDef {
    label: "Ctrl+D",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: agent_guard_matches,
    agent: None,
};

static SYSTEM_MENU: ChordDef = ChordDef {
    label: "Alt+Space",
    shell_too: false,
    under_modal: UnderModal::Act,
    matches: system_menu_matches,
    agent: None,
};

static PANEL: ChordDef = ChordDef {
    label: "Ctrl+Shift+P",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: panel_matches,
    agent: None,
};

fn board_matches(e: &KeyEvent, _c: &KeyContext) -> bool {
    ctrl(e) && e.shift && letter(e, 'a')
}

static BOARD: ChordDef = ChordDef {
    label: "Ctrl+Shift+A",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: board_matches,
    agent: None,
};

static PANEL_FOCUS: ChordDef = ChordDef {
    label: "Ctrl+Shift+O",
    shell_too: false,
    under_modal: UnderModal::Swallow,
    matches: panel_focus_matches,
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
        ChordId::Board => &BOARD,
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

/// What the UI calls a chord: its agent key where agent keys are on and it has one.
pub fn key_label(id: ChordId, agent_keys: bool) -> &'static str {
    let def = chord(id);
    match (&def.agent, agent_keys) {
        (Some(a), true) => a.label,
        _ => def.label,
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
    fn the_board_chord_is_ctrl_shift_a_and_plain_ctrl_a_stays_the_shells() {
        let c = on();
        let a = KeyEvent::new("A", "KeyA").ctrl().shift();
        assert_eq!(chord_of(&a, &c), Some(ChordId::Board));
        assert!(app_owns_key(&a, &c));
        // The physical key counts when a layout moves the letter (rule 11).
        assert_eq!(
            chord_of(&KeyEvent::new("\u{444}", "KeyA").ctrl().shift(), &c),
            Some(ChordId::Board)
        );
        // Readline's beginning-of-line, and Alt-held (AltGr) forms, are the shell's.
        for shells in [
            KeyEvent::new("a", "KeyA").ctrl(),
            KeyEvent::new("A", "KeyA").ctrl().shift().alt(),
            KeyEvent::new("A", "KeyA").shift(),
        ] {
            assert_eq!(chord_of(&shells, &c), None);
            assert!(!app_owns_key(&shells, &c));
        }
        assert_eq!(key_label(ChordId::Board, false), "Ctrl+Shift+A");
        assert_eq!(key_label(ChordId::Board, true), "Ctrl+Shift+A");
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
}
