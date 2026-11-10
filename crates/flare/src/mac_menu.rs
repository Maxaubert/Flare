//! The Mac menu bar's content (port phase 3, step 2): Flare, File, Edit, View, Window and Help, as
//! a `menu_spec` list that `flare_win::menu_bar` builds on macOS. Pure, so it is tested on every
//! system.
//!
//! An item with a key is that key press: choosing it runs the same path a typed key runs
//! (`window::handle_key`: the chord table, the terminal's copy and paste rules, a Slint field that
//! has the keys), and its key equivalent is that very key, so the menu and the keyboard can never
//! disagree. AppKit hands a key equivalent to the menu before the window sees it; a key the menu
//! does not catch (another layout's plus key) still reaches the keymap, which answers to it too.
//! The other items are the app's own (Quit through the window's close route, Command help) or
//! standard AppKit actions (Hide, Minimize, Zoom, Bring All to Front, the About panel).
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use flare_term::keys::KeyEvent;
use flare_win::menu_spec::{Entry, ItemAction, ItemSpec, MenuRole, MenuSpec, Mods, item, native};

/// What an app item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    // Keys: each acts as its key press.
    Settings,
    NewTab,
    CloseTab,
    Copy,
    Paste,
    PastePlain,
    SelectAll,
    Find,
    FindTab,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    FullScreen,
    PrevTab,
    NextTab,
    /// Tab 1 to 9.
    Tab(u8),
    // The app's own.
    Quit,
    CommandHelp,
}

/// Every app command, in menu order (the tag is the place in this list).
pub const ALL: [MenuCmd; 26] = [
    MenuCmd::Settings,
    MenuCmd::Quit,
    MenuCmd::NewTab,
    MenuCmd::CloseTab,
    MenuCmd::Copy,
    MenuCmd::Paste,
    MenuCmd::PastePlain,
    MenuCmd::SelectAll,
    MenuCmd::Find,
    MenuCmd::FindTab,
    MenuCmd::ZoomIn,
    MenuCmd::ZoomOut,
    MenuCmd::ZoomReset,
    MenuCmd::FullScreen,
    MenuCmd::PrevTab,
    MenuCmd::NextTab,
    MenuCmd::Tab(1),
    MenuCmd::Tab(2),
    MenuCmd::Tab(3),
    MenuCmd::Tab(4),
    MenuCmd::Tab(5),
    MenuCmd::Tab(6),
    MenuCmd::Tab(7),
    MenuCmd::Tab(8),
    MenuCmd::Tab(9),
    MenuCmd::CommandHelp,
];

/// The item's tag (its place in `ALL`, from 1: AppKit's default tag is 0).
pub fn tag(cmd: MenuCmd) -> isize {
    ALL.iter()
        .position(|c| *c == cmd)
        .map_or(0, |i| i as isize + 1)
}

/// The command a tag stands for.
pub fn from_tag(tag: isize) -> Option<MenuCmd> {
    usize::try_from(tag - 1)
        .ok()
        .and_then(|i| ALL.get(i).copied())
}

/// The key press a key command is, as the Mac keyboard makes it (Cmd is `meta`); None for the
/// app's own commands.
pub fn key_event(cmd: MenuCmd) -> Option<KeyEvent> {
    let cmd_key = |key: &str, code: &str| KeyEvent::new(key, code).meta();
    Some(match cmd {
        MenuCmd::Settings => cmd_key(",", "Comma"),
        MenuCmd::NewTab => cmd_key("t", "KeyT"),
        MenuCmd::CloseTab => cmd_key("w", "KeyW"),
        MenuCmd::Copy => cmd_key("c", "KeyC"),
        MenuCmd::Paste => cmd_key("v", "KeyV"),
        MenuCmd::PastePlain => cmd_key("V", "KeyV").shift(),
        MenuCmd::SelectAll => cmd_key("a", "KeyA"),
        MenuCmd::Find => cmd_key("f", "KeyF"),
        MenuCmd::FindTab => cmd_key("K", "KeyK").shift(),
        MenuCmd::ZoomIn => cmd_key("+", "Equal"),
        MenuCmd::ZoomOut => cmd_key("-", "Minus"),
        MenuCmd::ZoomReset => cmd_key("0", "Digit0"),
        MenuCmd::FullScreen => cmd_key("f", "KeyF").ctrl(),
        MenuCmd::PrevTab => cmd_key("{", "BracketLeft").shift(),
        MenuCmd::NextTab => cmd_key("}", "BracketRight").shift(),
        MenuCmd::Tab(n) => cmd_key(&n.to_string(), &format!("Digit{n}")),
        MenuCmd::Quit | MenuCmd::CommandHelp => return None,
    })
}

/// The key equivalent AppKit shows and matches for a key press: the character (a letter in lower
/// case, Shift in the mask) and the modifiers. A shifted symbol ("{", "+") is its own character, as
/// AppKit takes it, with Shift left out of the mask.
pub fn key_equivalent(e: &KeyEvent) -> (String, Mods) {
    let letter = e.key.len() == 1 && e.key.as_bytes()[0].is_ascii_alphabetic();
    let key = if letter {
        e.key.to_ascii_lowercase()
    } else {
        e.key.clone()
    };
    let mods = Mods {
        cmd: e.meta,
        shift: e.shift && letter,
        option: e.alt,
        ctrl: e.ctrl,
    };
    (key, mods)
}

/// An app item: its key equivalent comes from its key press.
fn app(title: &str, cmd: MenuCmd) -> Entry {
    let (key, mods) = key_event(cmd).map_or((String::new(), Mods::NONE), |e| key_equivalent(&e));
    item(title, &key, mods, tag(cmd))
}

fn menu(title: &str, role: MenuRole, entries: Vec<Entry>) -> MenuSpec {
    MenuSpec {
        title: title.to_string(),
        role,
        entries,
    }
}

/// The whole bar.
pub fn main_menu() -> Vec<MenuSpec> {
    let mut window = vec![
        native("Minimize", "m", Mods::CMD, "performMiniaturize:"),
        native("Zoom", "", Mods::NONE, "performZoom:"),
        Entry::Separator,
        app("Show Previous Tab", MenuCmd::PrevTab),
        app("Show Next Tab", MenuCmd::NextTab),
        Entry::Separator,
    ];
    for n in 1..=9u8 {
        window.push(app(&format!("Tab {n}"), MenuCmd::Tab(n)));
    }
    window.push(Entry::Separator);
    window.push(native(
        "Bring All to Front",
        "",
        Mods::NONE,
        "arrangeInFront:",
    ));
    vec![
        menu(
            "Flare",
            MenuRole::App,
            vec![
                native(
                    "About Flare",
                    "",
                    Mods::NONE,
                    "orderFrontStandardAboutPanel:",
                ),
                Entry::Separator,
                app("Settings\u{2026}", MenuCmd::Settings),
                Entry::Separator,
                Entry::Item(ItemSpec {
                    title: "Services".to_string(),
                    key: String::new(),
                    mods: Mods::NONE,
                    action: ItemAction::Services,
                }),
                Entry::Separator,
                native("Hide Flare", "h", Mods::CMD, "hide:"),
                native(
                    "Hide Others",
                    "h",
                    Mods::CMD_OPTION,
                    "hideOtherApplications:",
                ),
                native("Show All", "", Mods::NONE, "unhideAllApplications:"),
                Entry::Separator,
                item("Quit Flare", "q", Mods::CMD, tag(MenuCmd::Quit)),
            ],
        ),
        menu(
            "File",
            MenuRole::Plain,
            vec![
                app("New Tab", MenuCmd::NewTab),
                Entry::Separator,
                app("Close Tab", MenuCmd::CloseTab),
            ],
        ),
        menu(
            "Edit",
            MenuRole::Plain,
            vec![
                app("Copy", MenuCmd::Copy),
                app("Paste", MenuCmd::Paste),
                app("Paste as Plain Text", MenuCmd::PastePlain),
                app("Select All", MenuCmd::SelectAll),
                Entry::Separator,
                app("Find\u{2026}", MenuCmd::Find),
                app("Find a Tab\u{2026}", MenuCmd::FindTab),
            ],
        ),
        menu(
            "View",
            MenuRole::Plain,
            vec![
                app("Zoom In", MenuCmd::ZoomIn),
                app("Zoom Out", MenuCmd::ZoomOut),
                app("Actual Size", MenuCmd::ZoomReset),
                Entry::Separator,
                app("Enter Full Screen", MenuCmd::FullScreen),
            ],
        ),
        menu("Window", MenuRole::Window, window),
        menu(
            "Help",
            MenuRole::Help,
            vec![app("Command Help", MenuCmd::CommandHelp)],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_tabs::keymap::{ChordId, KeyContext, KeyOs, chord_of};
    use flare_term::keys::{KeyState, TermKeyAction, term_key_action_on};

    fn mac() -> KeyContext {
        KeyContext {
            os: KeyOs::Mac,
            help: true,
            vertical: true,
            ..KeyContext::default()
        }
    }

    fn find<'a>(bar: &'a [MenuSpec], title: &str) -> &'a ItemSpec {
        bar.iter()
            .flat_map(|m| m.items())
            .find(|i| i.title == title)
            .unwrap_or_else(|| panic!("no item {title}"))
    }

    fn term_action(e: &KeyEvent, sel: bool) -> TermKeyAction {
        let mut d = || false;
        let h = |_: &KeyEvent| false;
        let mut s = KeyState {
            has_selection: sel,
            delete_selection: &mut d,
            host_owns: &h,
            agent_here: false,
        };
        term_key_action_on(KeyOs::Mac, e, &mut s)
    }

    #[test]
    fn the_bar_has_the_mac_menus_in_order() {
        let bar = main_menu();
        let titles: Vec<&str> = bar.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Flare", "File", "Edit", "View", "Window", "Help"]
        );
        assert_eq!(bar[0].role, MenuRole::App);
        assert_eq!(bar[4].role, MenuRole::Window);
        assert_eq!(bar[5].role, MenuRole::Help);
    }

    #[test]
    fn the_standard_shortcuts_are_there() {
        let bar = main_menu();
        let cases = [
            ("Settings\u{2026}", ",", Mods::CMD),
            ("Hide Flare", "h", Mods::CMD),
            ("Hide Others", "h", Mods::CMD_OPTION),
            ("Quit Flare", "q", Mods::CMD),
            ("New Tab", "t", Mods::CMD),
            ("Close Tab", "w", Mods::CMD),
            ("Copy", "c", Mods::CMD),
            ("Paste", "v", Mods::CMD),
            ("Paste as Plain Text", "v", Mods::CMD_SHIFT),
            ("Select All", "a", Mods::CMD),
            ("Find\u{2026}", "f", Mods::CMD),
            ("Find a Tab\u{2026}", "k", Mods::CMD_SHIFT),
            ("Zoom In", "+", Mods::CMD),
            ("Zoom Out", "-", Mods::CMD),
            ("Actual Size", "0", Mods::CMD),
            ("Enter Full Screen", "f", Mods::CTRL_CMD),
            ("Minimize", "m", Mods::CMD),
            ("Show Previous Tab", "{", Mods::CMD),
            ("Show Next Tab", "}", Mods::CMD),
            ("Tab 1", "1", Mods::CMD),
            ("Tab 9", "9", Mods::CMD),
        ];
        for (title, key, mods) in cases {
            let i = find(&bar, title);
            assert_eq!((i.key.as_str(), i.mods), (key, mods), "{title}");
        }
    }

    #[test]
    fn every_key_item_is_the_chord_or_terminal_key_it_names() {
        let c = mac();
        let chords = [
            (MenuCmd::Settings, ChordId::Settings),
            (MenuCmd::NewTab, ChordId::NewTab),
            (MenuCmd::CloseTab, ChordId::CloseTab),
            (MenuCmd::Find, ChordId::Find),
            (MenuCmd::FindTab, ChordId::FindTab),
            (MenuCmd::ZoomIn, ChordId::ZoomIn),
            (MenuCmd::ZoomOut, ChordId::ZoomOut),
            (MenuCmd::ZoomReset, ChordId::ZoomReset),
            (MenuCmd::FullScreen, ChordId::Fullscreen),
            (MenuCmd::PrevTab, ChordId::NextTab),
            (MenuCmd::NextTab, ChordId::NextTab),
        ];
        for (cmd, id) in chords {
            let e = key_event(cmd).expect("a key item");
            assert_eq!(chord_of(&e, &c), Some(id), "{cmd:?}");
        }
        for n in 1..=9 {
            let e = key_event(MenuCmd::Tab(n)).expect("a key item");
            assert_eq!(chord_of(&e, &c), Some(ChordId::JumpTab), "tab {n}");
            assert_eq!(e.key.parse::<u8>().ok(), Some(n));
        }
        // Copy, paste and select all are the terminal's keys, never chords.
        let term = [
            (MenuCmd::Copy, true, TermKeyAction::Copy),
            (MenuCmd::Paste, false, TermKeyAction::Paste),
            (MenuCmd::PastePlain, false, TermKeyAction::PastePlain),
            (MenuCmd::SelectAll, false, TermKeyAction::SelectAll),
        ];
        for (cmd, sel, action) in term {
            let e = key_event(cmd).expect("a key item");
            assert_eq!(chord_of(&e, &c), None, "{cmd:?}");
            assert_eq!(term_action(&e, sel), action, "{cmd:?}");
        }
    }

    #[test]
    fn no_key_item_is_a_chord_on_windows() {
        // The menu is Mac only; its Cmd keys mean nothing to the Windows table.
        let w = KeyContext {
            help: true,
            vertical: true,
            ..KeyContext::default()
        };
        for cmd in ALL {
            if let Some(e) = key_event(cmd) {
                assert_eq!(chord_of(&e, &w), None, "{cmd:?}");
            }
        }
    }

    #[test]
    fn the_shown_shortcut_is_the_keymaps_label() {
        use flare_tabs::keymap::key_label_on;
        let bar = main_menu();
        let shown = |title: &str| {
            let i = find(&bar, title);
            format!("{}{}", i.mods.glyphs(), i.key.to_uppercase())
        };
        let label = |id| key_label_on(KeyOs::Mac, id, false);
        assert_eq!(shown("New Tab"), label(ChordId::NewTab));
        assert_eq!(shown("Close Tab"), label(ChordId::CloseTab));
        assert_eq!(shown("Settings\u{2026}"), label(ChordId::Settings));
        assert_eq!(shown("Find\u{2026}"), label(ChordId::Find));
        assert_eq!(shown("Find a Tab\u{2026}"), label(ChordId::FindTab));
        assert_eq!(shown("Zoom In"), label(ChordId::ZoomIn));
        assert_eq!(shown("Zoom Out"), label(ChordId::ZoomOut));
        assert_eq!(shown("Actual Size"), label(ChordId::ZoomReset));
        assert_eq!(shown("Enter Full Screen"), label(ChordId::Fullscreen));
    }

    #[test]
    fn every_tag_names_its_command_and_every_command_has_one_item() {
        let bar = main_menu();
        let tags: Vec<isize> = bar
            .iter()
            .flat_map(|m| m.items())
            .filter_map(|i| match i.action {
                ItemAction::Tag(t) => Some(t),
                _ => None,
            })
            .collect();
        for cmd in ALL {
            let t = tag(cmd);
            assert!(t > 0, "{cmd:?}");
            assert_eq!(from_tag(t), Some(cmd));
            assert_eq!(tags.iter().filter(|x| **x == t).count(), 1, "{cmd:?}");
        }
        assert_eq!(tags.len(), ALL.len());
        assert_eq!(from_tag(0), None);
        assert_eq!(from_tag(ALL.len() as isize + 1), None);
    }

    #[test]
    fn the_standard_items_send_appkits_own_actions() {
        let bar = main_menu();
        let sel = |title: &str| match &find(&bar, title).action {
            ItemAction::Selector(s) => *s,
            other => panic!("{title}: {other:?}"),
        };
        assert_eq!(sel("About Flare"), "orderFrontStandardAboutPanel:");
        assert_eq!(sel("Hide Flare"), "hide:");
        assert_eq!(sel("Hide Others"), "hideOtherApplications:");
        assert_eq!(sel("Show All"), "unhideAllApplications:");
        assert_eq!(sel("Minimize"), "performMiniaturize:");
        assert_eq!(sel("Zoom"), "performZoom:");
        assert_eq!(sel("Bring All to Front"), "arrangeInFront:");
        assert_eq!(find(&bar, "Services").action, ItemAction::Services);
    }

    #[test]
    fn no_key_is_used_twice_and_no_title_has_an_em_dash() {
        let bar = main_menu();
        let mut seen = std::collections::HashSet::new();
        for i in bar.iter().flat_map(|m| m.items()) {
            assert!(!i.title.contains('\u{2014}'), "{}", i.title);
            if !i.key.is_empty() {
                let k = (i.key.clone(), i.mods.glyphs());
                // Hide and Hide Others share the letter with different modifiers.
                assert!(seen.insert(k.clone()), "{} repeats {k:?}", i.title);
            }
        }
    }
}
