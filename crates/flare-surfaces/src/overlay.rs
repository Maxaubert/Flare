//! The airspace decision: which of Flare's overlays hide a web panel.
//!
//! A web panel is a child window, and a child window paints above everything Slint draws. So while
//! any overlay is open the web panel is hidden and a still of its last frame is drawn in its
//! place. Native panels are Slint and need none of this.
//!
//! The window computes one Slint property, `overlay-open`, from the same list. `Overlay::ALL` is
//! the list; a test in `flare` opens each one and checks the property, and another checks that the
//! `.slint` file mentions every name here, so a new overlay cannot be added without being counted.

/// Every overlay that can cover the panel slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overlay {
    /// A right-click menu.
    Menu,
    /// The tab list (overflow) popup.
    Overflow,
    /// The close question and the other dialogs.
    Dialog,
    /// The update window.
    UpdateWindow,
    /// The notice hung under the update chip.
    UpdateNotice,
    /// The Settings page is in front (it covers the page, panel included).
    Settings,
    /// The Settings dropdown.
    SettingsSelect,
    /// The theme editor.
    ThemeEditor,
    /// The three-way question of the theme switch.
    SettingsAsk,
    /// The Dictation page's question.
    DictationAsk,
    /// The colour picker.
    ColourPicker,
    /// The Command help panel.
    Help,
    /// The find bar.
    Find,
}

impl Overlay {
    /// All of them.
    pub const ALL: [Overlay; 13] = [
        Overlay::Menu,
        Overlay::Overflow,
        Overlay::Dialog,
        Overlay::UpdateWindow,
        Overlay::UpdateNotice,
        Overlay::Settings,
        Overlay::SettingsSelect,
        Overlay::ThemeEditor,
        Overlay::SettingsAsk,
        Overlay::DictationAsk,
        Overlay::ColourPicker,
        Overlay::Help,
        Overlay::Find,
    ];

    /// The name the `.slint` file uses for this overlay's term in `overlay-open`.
    pub fn slint_name(self) -> &'static str {
        match self {
            Overlay::Menu => "menu-open",
            Overlay::Overflow => "overflow-open",
            Overlay::Dialog => "dialog-open",
            Overlay::UpdateWindow => "update-open",
            Overlay::UpdateNotice => "update-notice",
            Overlay::Settings => "settings-front",
            Overlay::SettingsSelect => "select-open",
            Overlay::ThemeEditor => "editor-open",
            Overlay::SettingsAsk => "ask-open",
            Overlay::DictationAsk => "dict-ask-open",
            Overlay::ColourPicker => "picker-open",
            Overlay::Help => "help-open",
            Overlay::Find => "find-open",
        }
    }
}

/// Which overlays are open now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlays {
    open: Vec<Overlay>,
}

impl Overlays {
    pub fn set(&mut self, which: Overlay, on: bool) {
        self.open.retain(|o| *o != which);
        if on {
            self.open.push(which);
        }
    }

    pub fn is_open(&self, which: Overlay) -> bool {
        self.open.contains(&which)
    }

    /// True while any overlay is open: what the `overlay-open` property says.
    pub fn any(&self) -> bool {
        !self.open.is_empty()
    }
}

/// What a panel does about the overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDraw {
    /// Drawn as usual.
    Live,
    /// A web panel's window is hidden and a still of its last frame is drawn in its place.
    Still,
}

/// The decision: a native panel is never hidden (Slint orders it with the rest), a web panel is
/// hidden while any overlay is open.
pub fn draw_for(native: bool, overlays: &Overlays) -> PanelDraw {
    if native || !overlays.any() {
        PanelDraw::Live
    } else {
        PanelDraw::Still
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_open_draws_live() {
        let o = Overlays::default();
        assert!(!o.any());
        assert_eq!(draw_for(false, &o), PanelDraw::Live);
    }

    #[test]
    fn any_single_overlay_hides_a_web_panel() {
        for which in Overlay::ALL {
            let mut o = Overlays::default();
            o.set(which, true);
            assert!(o.any() && o.is_open(which));
            assert_eq!(draw_for(false, &o), PanelDraw::Still, "{which:?}");
            o.set(which, false);
            assert_eq!(draw_for(false, &o), PanelDraw::Live, "{which:?}");
        }
    }

    #[test]
    fn a_native_panel_is_never_hidden() {
        let mut o = Overlays::default();
        for which in Overlay::ALL {
            o.set(which, true);
        }
        assert_eq!(draw_for(true, &o), PanelDraw::Live);
    }

    #[test]
    fn two_overlays_need_both_closed() {
        let mut o = Overlays::default();
        o.set(Overlay::Menu, true);
        o.set(Overlay::Find, true);
        o.set(Overlay::Menu, false);
        assert_eq!(draw_for(false, &o), PanelDraw::Still);
        o.set(Overlay::Find, false);
        assert_eq!(draw_for(false, &o), PanelDraw::Live);
    }

    #[test]
    fn setting_twice_counts_once() {
        let mut o = Overlays::default();
        o.set(Overlay::Dialog, true);
        o.set(Overlay::Dialog, true);
        o.set(Overlay::Dialog, false);
        assert!(!o.any());
    }

    #[test]
    fn the_list_and_the_names_are_complete_and_distinct() {
        let mut names: Vec<&str> = Overlay::ALL.iter().map(|o| o.slint_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Overlay::ALL.len());
    }
}
