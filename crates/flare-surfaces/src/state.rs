//! The per-tab panel state: what each tab shows beside its terminal and how wide.

use crate::kind::PanelKind;
use std::collections::HashMap;

/// The longest target kept (a path or an address).
pub const TARGET_MAX: usize = 2048;

/// One tab's panel. `open` false keeps what it showed last, so the toggle chord can bring it back.
#[derive(Clone, Debug, PartialEq)]
pub struct TabPanel {
    pub kind: PanelKind,
    /// The file or address shown. Never empty.
    pub target: String,
    pub open: bool,
    /// The slot's share of the page width. None until the user drags the splitter (never set is
    /// not zero: the layout's default applies, rule 40).
    pub width: Option<f32>,
}

impl TabPanel {
    pub fn new(kind: PanelKind, target: &str) -> Option<TabPanel> {
        let target = clean_target(target)?;
        Some(TabPanel {
            kind,
            target,
            open: true,
            width: None,
        })
    }
}

/// A target is non-empty, has no NUL and is not over-long.
pub fn clean_target(t: &str) -> Option<String> {
    (!t.is_empty() && t.chars().count() <= TARGET_MAX && !t.contains('\0')).then(|| t.to_string())
}

/// Every tab's panel, by tab id.
#[derive(Debug, Default)]
pub struct TabPanels {
    by_tab: HashMap<String, TabPanel>,
}

impl TabPanels {
    pub fn get(&self, tab: &str) -> Option<&TabPanel> {
        self.by_tab.get(tab)
    }

    /// The panel that is OPEN on this tab.
    pub fn open_on(&self, tab: &str) -> Option<&TabPanel> {
        self.get(tab).filter(|p| p.open)
    }

    /// Shows `kind` / `target` on the tab and opens it. The width the tab already had is kept.
    /// Returns false for an unusable target.
    pub fn show(&mut self, tab: &str, kind: PanelKind, target: &str) -> bool {
        let Some(target) = clean_target(target) else {
            return false;
        };
        let width = self.by_tab.get(tab).and_then(|p| p.width);
        self.by_tab.insert(
            tab.to_string(),
            TabPanel {
                kind,
                target,
                open: true,
                width,
            },
        );
        true
    }

    /// Changes what the tab's panel points at without opening, closing or moving it (a web page the
    /// user navigated). False when the tab has no panel or the target is unusable.
    pub fn set_target(&mut self, tab: &str, target: &str) -> bool {
        let Some(target) = clean_target(target) else {
            return false;
        };
        match self.by_tab.get_mut(tab) {
            Some(p) => {
                p.target = target;
                true
            }
            None => false,
        }
    }

    /// Closes the tab's panel (what it showed stays for the toggle chord). False when there was
    /// nothing open.
    pub fn close(&mut self, tab: &str) -> bool {
        match self.by_tab.get_mut(tab) {
            Some(p) if p.open => {
                p.open = false;
                true
            }
            _ => false,
        }
    }

    /// Opens again what the tab last showed. False when it showed nothing or is already open.
    pub fn reopen(&mut self, tab: &str) -> bool {
        match self.by_tab.get_mut(tab) {
            Some(p) if !p.open => {
                p.open = true;
                true
            }
            _ => false,
        }
    }

    /// Stores the splitter's share. A value that is not a fraction of the page is ignored.
    pub fn set_width(&mut self, tab: &str, frac: f32) -> bool {
        match self.by_tab.get_mut(tab) {
            Some(p) if frac.is_finite() && frac > 0.0 && frac < 1.0 => {
                p.width = Some(frac);
                true
            }
            _ => false,
        }
    }

    /// The tab is gone.
    pub fn forget(&mut self, tab: &str) -> bool {
        self.by_tab.remove(tab).is_some()
    }

    /// Puts a restored panel on a tab (the store's answer), replacing nothing the user did since.
    pub fn restore(&mut self, tab: &str, panel: TabPanel) {
        self.by_tab.entry(tab.to_string()).or_insert(panel);
    }

    pub fn is_empty(&self) -> bool {
        self.by_tab.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_tab.len()
    }

    /// How many panels are open (a closed panel costs nothing).
    pub fn open_count(&self) -> usize {
        self.by_tab.values().filter(|p| p.open).count()
    }

    /// The tabs whose OPEN panel shows the conversation (Changes or Tasks), with the kind. Only these tabs need a
    /// transcript tracker.
    pub fn open_convo(&self) -> Vec<(String, PanelKind)> {
        let mut v: Vec<(String, PanelKind)> = self
            .by_tab
            .iter()
            .filter(|(_, p)| p.open && p.kind.is_convo())
            .map(|(t, p)| (t.clone(), p.kind.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> PanelKind {
        PanelKind::Image
    }

    #[test]
    fn show_opens_and_close_keeps_what_was_shown() {
        let mut p = TabPanels::default();
        assert!(p.show("t1", img(), r"C:\a.png"));
        assert!(p.open_on("t1").is_some());
        assert!(p.close("t1"));
        assert!(p.open_on("t1").is_none());
        assert_eq!(p.get("t1").map(|x| x.target.as_str()), Some(r"C:\a.png"));
        assert!(!p.close("t1"));
        assert!(p.reopen("t1"));
        assert!(!p.reopen("t1"));
        assert!(!p.reopen("nope"));
    }

    #[test]
    fn a_new_file_keeps_the_tabs_width() {
        let mut p = TabPanels::default();
        p.show("t1", img(), "a.png");
        assert!(p.set_width("t1", 0.3));
        p.show("t1", PanelKind::Text, "b.txt");
        assert_eq!(p.get("t1").and_then(|x| x.width), Some(0.3));
        assert_eq!(p.get("t1").map(|x| x.kind.clone()), Some(PanelKind::Text));
    }

    #[test]
    fn set_target_follows_a_page_without_reopening_it() {
        let mut p = TabPanels::default();
        p.show("t1", PanelKind::Web, "https://a.example/");
        p.set_width("t1", 0.3);
        assert!(p.set_target("t1", "https://b.example/"));
        let t = p.get("t1").unwrap();
        assert_eq!(
            (t.target.as_str(), t.open, t.width),
            ("https://b.example/", true, Some(0.3))
        );
        assert!(!p.set_target("t1", ""));
        assert!(!p.set_target("nope", "https://c.example/"));
    }

    #[test]
    fn a_width_that_is_not_a_fraction_is_ignored() {
        let mut p = TabPanels::default();
        p.show("t1", img(), "a.png");
        for bad in [0.0, 1.0, -0.2, 1.5, f32::NAN, f32::INFINITY] {
            assert!(!p.set_width("t1", bad), "{bad}");
        }
        assert_eq!(p.get("t1").and_then(|x| x.width), None);
        assert!(!p.set_width("nope", 0.5));
    }

    #[test]
    fn an_unusable_target_shows_nothing() {
        let mut p = TabPanels::default();
        assert!(!p.show("t1", img(), ""));
        assert!(!p.show("t1", img(), "a\0b"));
        assert!(!p.show("t1", img(), &"x".repeat(TARGET_MAX + 1)));
        assert!(p.is_empty());
        assert!(TabPanel::new(img(), "").is_none());
    }

    #[test]
    fn only_open_conversation_panels_are_listed_for_a_tracker() {
        let mut p = TabPanels::default();
        p.show("t1", PanelKind::Changes, "changes");
        p.show("t2", PanelKind::Tasks, "tasks");
        p.show("t3", img(), "a.png");
        p.show("t4", PanelKind::Changes, "changes");
        p.close("t4");
        assert_eq!(
            p.open_convo(),
            vec![
                ("t1".to_string(), PanelKind::Changes),
                ("t2".to_string(), PanelKind::Tasks)
            ]
        );
        p.reopen("t4");
        assert_eq!(p.open_convo().len(), 3);
    }

    #[test]
    fn forget_drops_the_tab_and_counts_follow() {
        let mut p = TabPanels::default();
        p.show("t1", img(), "a.png");
        p.show("t2", img(), "b.png");
        p.close("t2");
        assert_eq!((p.len(), p.open_count()), (2, 1));
        assert!(p.forget("t1"));
        assert!(!p.forget("t1"));
        assert_eq!((p.len(), p.open_count()), (1, 0));
    }

    #[test]
    fn a_restored_panel_never_replaces_what_the_user_did_since() {
        let mut p = TabPanels::default();
        p.show("t1", img(), "new.png");
        p.restore("t1", TabPanel::new(img(), "old.png").unwrap());
        assert_eq!(p.get("t1").map(|x| x.target.as_str()), Some("new.png"));
        p.restore("t2", TabPanel::new(img(), "old.png").unwrap());
        assert!(p.get("t2").is_some());
    }
}
