//! Where a browser view sits (plugins, Phase 2b): a slot is an owner and a kind. A tab's panel is the slot
//! `{tab, panel}`, a session bar beside a tab is `{tab, bar}`, and a plugin's background page is `{plugin id,
//! background}`, tied to no tab. The browser host and the mods controller key their views and pages by slot, so
//! a plugin can have a view that is not a tab's panel.
//!
//! Only the panel kind is made by the app today; the bar and the background page come in later phases.

/// What a view in a slot is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewKind {
    /// The panel beside a tab's terminal (the web panel, a mini-app, a mod's page).
    Panel,
    /// A plugin's thin bar beside a tab (the session bar).
    Bar,
    /// A plugin's hidden page, tied to no tab.
    Background,
}

impl ViewKind {
    /// The word a log prints.
    pub fn name(self) -> &'static str {
        match self {
            ViewKind::Panel => "panel",
            ViewKind::Bar => "bar",
            ViewKind::Background => "background",
        }
    }
}

/// A view's place: who owns it (a tab id for a panel and a bar, a plugin id for a background page) and what it is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot {
    pub owner: String,
    pub kind: ViewKind,
}

impl Slot {
    /// The tab's panel.
    pub fn panel(tab: &str) -> Self {
        Self {
            owner: tab.to_string(),
            kind: ViewKind::Panel,
        }
    }

    /// The bar beside the tab.
    pub fn bar(tab: &str) -> Self {
        Self {
            owner: tab.to_string(),
            kind: ViewKind::Bar,
        }
    }

    /// The plugin's background page.
    pub fn background(plugin: &str) -> Self {
        Self {
            owner: plugin.to_string(),
            kind: ViewKind::Background,
        }
    }

    /// The tab the view sits in: the owner of a panel or a bar, none for a background page.
    pub fn tab(&self) -> Option<&str> {
        match self.kind {
            ViewKind::Panel | ViewKind::Bar => Some(&self.owner),
            ViewKind::Background => None,
        }
    }

    pub fn is_panel(&self) -> bool {
        self.kind == ViewKind::Panel
    }
}

/// A bare tab id means that tab's panel, the one slot every caller had before slots existed.
impl From<&str> for Slot {
    fn from(tab: &str) -> Self {
        Slot::panel(tab)
    }
}

impl From<&String> for Slot {
    fn from(tab: &String) -> Self {
        Slot::panel(tab)
    }
}

impl From<&Slot> for Slot {
    fn from(s: &Slot) -> Self {
        s.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_and_a_bar_name_their_tab_and_a_background_page_names_none() {
        assert_eq!(Slot::panel("t1").tab(), Some("t1"));
        assert_eq!(Slot::bar("t1").tab(), Some("t1"));
        assert_eq!(Slot::background("notes").tab(), None);
        assert_eq!(Slot::background("notes").owner, "notes");
    }

    #[test]
    fn the_same_owner_in_two_kinds_is_two_slots() {
        assert_ne!(Slot::panel("x"), Slot::bar("x"));
        assert_ne!(Slot::panel("x"), Slot::background("x"));
        let set: std::collections::HashSet<Slot> =
            [Slot::panel("x"), Slot::bar("x"), Slot::background("x")].into();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn a_bare_tab_is_its_panel() {
        assert_eq!(Slot::from("t1"), Slot::panel("t1"));
        assert_eq!(Slot::from(&"t1".to_string()), Slot::panel("t1"));
        assert!(Slot::from("t1").is_panel());
        assert_eq!(ViewKind::Background.name(), "background");
    }
}
