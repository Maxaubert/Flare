//! The app menu bar as plain data (port phase 3): menus, items, their key equivalents and what
//! each item does. Pure, so the menu Flare builds is tested on every system; on macOS
//! `menu_bar::install` turns it into the `NSMenu` main menu. Windows has no menu bar (Flare's top
//! row carries the tabs and its own buttons), so nothing there reads it.

/// The modifiers of a key equivalent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub cmd: bool,
    pub shift: bool,
    pub option: bool,
    pub ctrl: bool,
}

impl Mods {
    pub const CMD: Mods = Mods {
        cmd: true,
        shift: false,
        option: false,
        ctrl: false,
    };
    pub const CMD_SHIFT: Mods = Mods {
        cmd: true,
        shift: true,
        option: false,
        ctrl: false,
    };
    pub const CMD_OPTION: Mods = Mods {
        cmd: true,
        shift: false,
        option: true,
        ctrl: false,
    };
    pub const CTRL_CMD: Mods = Mods {
        cmd: true,
        shift: false,
        option: false,
        ctrl: true,
    };
    pub const NONE: Mods = Mods {
        cmd: false,
        shift: false,
        option: false,
        ctrl: false,
    };

    /// The Mac shortcut text (Control, Option, Shift, Command, in Apple's order).
    pub fn glyphs(self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push('\u{2303}');
        }
        if self.option {
            s.push('\u{2325}');
        }
        if self.shift {
            s.push('\u{21e7}');
        }
        if self.cmd {
            s.push('\u{2318}');
        }
        s
    }
}

/// What choosing an item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemAction {
    /// The app's own: the callback gets this tag.
    Tag(isize),
    /// A standard AppKit action sent up the responder chain (`hide:`, `performMiniaturize:`).
    Selector(&'static str),
    /// The Services submenu, filled by the system.
    Services,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSpec {
    pub title: String,
    /// The key equivalent as AppKit takes it: a lower-case character, "" for none.
    pub key: String,
    pub mods: Mods,
    pub action: ItemAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item(ItemSpec),
    Separator,
}

/// What the system does with a menu beyond showing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    Plain,
    /// The first menu, titled by the app's name.
    App,
    /// The system adds the window list here.
    Window,
    /// The system adds its help search here.
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuSpec {
    pub title: String,
    pub role: MenuRole,
    pub entries: Vec<Entry>,
}

impl MenuSpec {
    /// The items, separators left out.
    pub fn items(&self) -> impl Iterator<Item = &ItemSpec> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            Entry::Separator => None,
        })
    }
}

/// An item that runs the app's action `tag`.
pub fn item(title: &str, key: &str, mods: Mods, tag: isize) -> Entry {
    Entry::Item(ItemSpec {
        title: title.to_string(),
        key: key.to_string(),
        mods,
        action: ItemAction::Tag(tag),
    })
}

/// An item that sends a standard AppKit action.
pub fn native(title: &str, key: &str, mods: Mods, selector: &'static str) -> Entry {
    Entry::Item(ItemSpec {
        title: title.to_string(),
        key: key.to_string(),
        mods,
        action: ItemAction::Selector(selector),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyphs_follow_apples_order() {
        assert_eq!(Mods::CMD.glyphs(), "\u{2318}");
        assert_eq!(Mods::CMD_SHIFT.glyphs(), "\u{21e7}\u{2318}");
        assert_eq!(Mods::CTRL_CMD.glyphs(), "\u{2303}\u{2318}");
        assert_eq!(Mods::CMD_OPTION.glyphs(), "\u{2325}\u{2318}");
        assert_eq!(Mods::NONE.glyphs(), "");
    }

    #[test]
    fn items_skip_the_separators() {
        let m = MenuSpec {
            title: "File".into(),
            role: MenuRole::Plain,
            entries: vec![
                item("New Tab", "t", Mods::CMD, 1),
                Entry::Separator,
                native("Minimize", "m", Mods::CMD, "performMiniaturize:"),
            ],
        };
        let titles: Vec<&str> = m.items().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["New Tab", "Minimize"]);
    }
}
