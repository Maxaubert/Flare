//! The airspace decision: which of Flare's overlays hide a web panel.
//!
//! A web panel is a child window, and a child window paints above everything Slint draws. So while
//! any overlay is open the web panel is hidden and a still of its last frame is drawn in its
//! place. Native panels are Slint and need none of this.
//!
//! The window computes one Slint property, `overlay-open`, from the same list. `Overlay::ALL` is
//! the list; a test in `flare` opens each one and checks the property, and the match in that test
//! has no catch-all arm, so a new overlay cannot be added without being counted.

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
    /// The F12 diagnostics card, drawn at the page's top right.
    Diagnostics,
    /// A tab being dragged: its ghost follows the pointer over the page.
    TabDrag,
    /// The collapsed sidebar, out on a hover, reaching over the panel's slot.
    SidebarOverSlot,
}

impl Overlay {
    /// All of them.
    pub const ALL: [Overlay; 16] = [
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
        Overlay::Diagnostics,
        Overlay::TabDrag,
        Overlay::SidebarOverSlot,
    ];
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
}
