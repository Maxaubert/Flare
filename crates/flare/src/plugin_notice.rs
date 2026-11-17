//! The plugins' notices under the title bar (`notice.show`, rule 1275, 2026-11-17), as the window keeps them: one a
//! plugin (a newer one takes its place and starts its time again), at most [`MAX_SHOWN`] in all (the oldest goes), each
//! gone after `flare_mods::notice::SHOW_FOR` of time with the pointer off it. Pure: the glue ticks it and draws it.

/// How often the glue counts down, while any notice shows.
pub const TICK_MS: u64 = 100;
/// Notices on screen at once; a further plugin's pushes the oldest out.
pub const MAX_SHOWN: usize = 4;

/// The time a notice shows, in ms.
pub fn show_ms() -> u64 {
    u64::try_from(flare_mods::notice::SHOW_FOR.as_millis()).unwrap_or(6_000)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub plugin: String,
    pub text: String,
    pub error: bool,
    /// Time left with the pointer off it.
    pub left_ms: u64,
    /// The pointer is on it: its time stands still.
    pub hovered: bool,
}

#[derive(Debug, Default)]
pub struct Notices {
    list: Vec<Notice>,
}

impl Notices {
    pub fn list(&self) -> &[Notice] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The plugin's notice, in place of the one it had (in its place, with its time new); else a new one, last.
    pub fn show(&mut self, plugin: &str, text: &str, error: bool) {
        if let Some(n) = self.list.iter_mut().find(|n| n.plugin == plugin) {
            n.text = text.to_string();
            n.error = error;
            n.left_ms = show_ms();
            return;
        }
        self.list.push(Notice {
            plugin: plugin.to_string(),
            text: text.to_string(),
            error,
            left_ms: show_ms(),
            hovered: false,
        });
        if self.list.len() > MAX_SHOWN {
            self.list.remove(0);
        }
    }

    /// The plugin's notice goes (its close button, or the plugin was turned off). True when it had one.
    pub fn close(&mut self, plugin: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|n| n.plugin != plugin);
        self.list.len() != before
    }

    /// The pointer came onto the plugin's notice or left it.
    pub fn hover(&mut self, plugin: &str, on: bool) {
        if let Some(n) = self.list.iter_mut().find(|n| n.plugin == plugin) {
            n.hovered = on;
        }
    }

    /// The window built its toasts anew (a new list): each new toast starts with the pointer off it and reports a hover
    /// only when the pointer moves onto it, so none counts as hovered until then. Without this, a toast that moved up
    /// from under the pointer (the one above it went) or was rebuilt as the pointer left would stand still for good.
    pub fn unhover_all(&mut self) {
        for n in &mut self.list {
            n.hovered = false;
        }
    }

    /// `ms` went by: each notice the pointer is not on loses them, and the ones out of time go. True when one went.
    pub fn tick(&mut self, ms: u64) -> bool {
        for n in self.list.iter_mut().filter(|n| !n.hovered) {
            n.left_ms = n.left_ms.saturating_sub(ms);
        }
        let before = self.list.len();
        self.list.retain(|n| n.left_ms > 0);
        self.list.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: &Notices) -> Vec<&str> {
        n.list().iter().map(|n| n.plugin.as_str()).collect()
    }

    #[test]
    fn a_notice_goes_after_six_seconds_and_stands_still_while_hovered() {
        let mut n = Notices::default();
        n.show("vault", "Could not lock", true);
        assert!(!n.tick(5_900));
        n.hover("vault", true);
        assert!(!n.tick(60_000), "the pointer is on it");
        n.hover("vault", false);
        assert!(!n.tick(50));
        assert!(n.tick(50), "6 s off the pointer in all");
        assert!(n.is_empty());
    }

    #[test]
    fn a_newer_notice_of_the_plugin_takes_its_place_with_its_time_new() {
        let mut n = Notices::default();
        n.show("a", "one", false);
        n.show("b", "two", false);
        n.tick(5_000);
        n.show("a", "three", true);
        assert_eq!(ids(&n), ["a", "b"], "in its place");
        assert_eq!(n.list()[0].text, "three");
        assert!(n.list()[0].error);
        assert!(n.tick(1_000), "b is out of time");
        assert_eq!(ids(&n), ["a"]);
        assert!(!n.tick(4_999));
        assert!(n.tick(1));
    }

    #[test]
    fn closing_takes_only_that_plugins_notice_and_the_oldest_goes_past_four() {
        let mut n = Notices::default();
        for p in ["a", "b", "c", "d", "e"] {
            n.show(p, p, false);
        }
        assert_eq!(ids(&n), ["b", "c", "d", "e"]);
        assert!(n.close("c"));
        assert!(!n.close("c"));
        assert!(!n.close("zz"));
        assert_eq!(ids(&n), ["b", "d", "e"]);
        n.hover("zz", true);
        assert!(n.list().iter().all(|x| !x.hovered));
    }

    /// The window's toasts are built anew with each new list, the pointer off each: no notice stays hovered past that.
    #[test]
    fn a_new_list_counts_no_notice_as_hovered() {
        let mut n = Notices::default();
        n.show("a", "one", false);
        n.show("b", "two", false);
        n.hover("b", true);
        n.unhover_all();
        assert!(n.list().iter().all(|x| !x.hovered));
        assert!(n.tick(show_ms()), "both run out");
        assert!(n.is_empty());
    }
}
