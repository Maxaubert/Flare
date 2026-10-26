//! The plugins' session bar (plugins phase 6, design 5.5 and 13), the pure part: which plugin's bar each tab shows,
//! and how tall it is. No Slint and no window: `appctl` keeps one [`Bars`], gives the front tab's height to the
//! layout (the terminal gives it up and reflows once) and its plugin's page to the browser.
//!
//! - A plugin that holds `bar` shows its strip under a tab's terminal with `bar.show {tab?}` and takes it away with
//!   `bar.hide {tab?}`. One bar a tab: while one plugin's shows, another plugin asking gets `busy`.
//! - The height is the manifest's `bar.height`, kept within [`MIN_HEIGHT`] and [`MAX_HEIGHT`].
//! - Every bar of a plugin goes when it stops (turned off, its background page ended), and a tab's goes with the tab.

use std::collections::HashMap;

/// The height of a bar, in logical pixels (the manifest's limits).
pub const MIN_HEIGHT: u32 = flare_mods::manifest::MIN_BAR_HEIGHT as u32;
pub const MAX_HEIGHT: u32 = flare_mods::manifest::MAX_BAR_HEIGHT as u32;
/// `bar.show` and `bar.hide` calls a second for one plugin on one tab, each (design 13).
pub const BAR_RATE: f64 = 1.0;
/// The plugin's `session-bar` actions drawn beside its bar at most.
pub const CHIPS_MAX: usize = 4;
/// Bar views one plugin keeps alive at most (design 13): the one in front and the last one before it; an older one
/// behind is closed and made again when its tab comes back.
pub const LIVE_PER_PLUGIN: usize = 2;
/// Plugin views in Flare at most, of every kind (design 13): the panels, the background pages and the bars. The bars
/// are what gives way: a bar view behind past this is closed, oldest first, and made again when its tab comes back.
pub const MAX_VIEWS: usize = 12;

/// The bar a tab shows: whose, and how tall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    pub plugin: String,
    pub height: u32,
}

/// Why `bar.show` changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Another plugin's bar shows on the tab (its id).
    Busy(String),
}

/// The bars the tabs show, by tab.
#[derive(Debug, Default)]
pub struct Bars {
    by_tab: HashMap<String, Hold>,
}

impl Bars {
    /// The plugin's bar on `tab`, `height` tall (clamped). True when what the tab shows changed; `Busy` when another
    /// plugin's bar is there.
    pub fn show(&mut self, tab: &str, plugin: &str, height: u32) -> Result<bool, Refused> {
        let hold = Hold {
            plugin: plugin.to_string(),
            height: height.clamp(MIN_HEIGHT, MAX_HEIGHT),
        };
        match self.by_tab.get(tab) {
            Some(h) if h.plugin != plugin => Err(Refused::Busy(h.plugin.clone())),
            Some(h) if *h == hold => Ok(false),
            _ => {
                self.by_tab.insert(tab.to_string(), hold);
                Ok(true)
            }
        }
    }

    /// The plugin's bar leaves `tab`. True when it showed there; another plugin's bar stays.
    pub fn hide(&mut self, tab: &str, plugin: &str) -> bool {
        if self.by_tab.get(tab).is_some_and(|h| h.plugin == plugin) {
            self.by_tab.remove(tab);
            return true;
        }
        false
    }

    /// The bar `tab` shows, if any.
    pub fn of(&self, tab: &str) -> Option<&Hold> {
        self.by_tab.get(tab)
    }

    /// The height the tab's terminal gives up (0: no bar).
    pub fn height(&self, tab: Option<&str>) -> u32 {
        tab.and_then(|t| self.of(t)).map_or(0, |h| h.height)
    }

    /// Every bar of the plugin goes (it stopped). The tabs that lost one, sorted.
    pub fn clear_plugin(&mut self, plugin: &str) -> Vec<String> {
        let mut gone: Vec<String> = self
            .by_tab
            .iter()
            .filter(|(_, h)| h.plugin == plugin)
            .map(|(t, _)| t.clone())
            .collect();
        gone.sort();
        for t in &gone {
            self.by_tab.remove(t);
        }
        gone
    }

    /// The tab closed: its bar goes. True when it had one.
    pub fn remove_tab(&mut self, tab: &str) -> bool {
        self.by_tab.remove(tab).is_some()
    }

    /// Every tab with a bar and the bar it shows, sorted by tab.
    pub fn all(&self) -> Vec<(String, Hold)> {
        let mut v: Vec<(String, Hold)> = self
            .by_tab
            .iter()
            .map(|(t, h)| (t.clone(), h.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn is_empty(&self) -> bool {
        self.by_tab.is_empty()
    }
}

/// Which of a plugin's live bar views to close so that at most [`LIVE_PER_PLUGIN`] stay: `live` is (tab, plugin, when
/// it was last on screen) for each view, `front` the tab on screen now (never closed). The oldest go first.
pub fn surplus(live: &[(String, String, u64)], front: Option<&str>) -> Vec<String> {
    let mut by_plugin: HashMap<&str, Vec<(&str, u64)>> = HashMap::new();
    for (tab, plugin, seen) in live {
        by_plugin
            .entry(plugin.as_str())
            .or_default()
            .push((tab.as_str(), *seen));
    }
    let mut out = Vec::new();
    for (_, mut views) in by_plugin {
        if views.len() <= LIVE_PER_PLUGIN {
            continue;
        }
        // Newest first; the front tab counts as the newest of all.
        views.sort_by_key(|(t, seen)| std::cmp::Reverse((Some(*t) == front, *seen)));
        out.extend(
            views
                .into_iter()
                .skip(LIVE_PER_PLUGIN)
                .map(|(t, _)| t.to_string()),
        );
    }
    out.sort();
    out
}

/// Which live bar views to close so that, with the `others` plugin views that are not bars (the panels and the
/// background pages), at most [`MAX_VIEWS`] stay: `live` as for [`surplus`]. The front tab's is never closed, so it is
/// the one bar that may stand past the total when the panels and background pages alone fill it. The oldest go first.
pub fn over_total(
    live: &[(String, String, u64)],
    front: Option<&str>,
    others: usize,
) -> Vec<String> {
    let room = MAX_VIEWS.saturating_sub(others);
    if live.len() <= room {
        return Vec::new();
    }
    let mut views: Vec<(&str, u64)> = live.iter().map(|(t, _, s)| (t.as_str(), *s)).collect();
    // Newest first; the front tab counts as the newest of all and always stays.
    views.sort_by_key(|(t, seen)| std::cmp::Reverse((Some(*t) == front, *seen)));
    let keep = room.max(usize::from(
        front.is_some_and(|f| views.first().is_some_and(|v| v.0 == f)),
    ));
    let mut out: Vec<String> = views
        .into_iter()
        .skip(keep)
        .map(|(t, _)| t.to_string())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bar_a_tab_and_a_second_plugin_is_busy() {
        let mut b = Bars::default();
        assert_eq!(b.show("t1", "player", 48), Ok(true));
        assert_eq!(
            b.show("t1", "player", 48),
            Ok(false),
            "the same again is no change"
        );
        assert_eq!(b.show("t1", "player", 60), Ok(true), "a new height is");
        assert_eq!(
            b.show("t1", "other", 30),
            Err(Refused::Busy("player".into()))
        );
        assert_eq!(b.of("t1").unwrap().plugin, "player");
        // Another tab is free.
        assert_eq!(b.show("t2", "other", 30), Ok(true));
        // Hiding another plugin's bar does nothing.
        assert!(!b.hide("t1", "other"));
        assert!(b.hide("t1", "player"));
        assert!(!b.hide("t1", "player"));
        assert_eq!(b.show("t1", "other", 30), Ok(true), "free again");
    }

    #[test]
    fn the_height_is_kept_within_the_manifests_limits() {
        let mut b = Bars::default();
        b.show("a", "p", 2).unwrap();
        b.show("b", "p", 900).unwrap();
        assert_eq!(b.height(Some("a")), MIN_HEIGHT);
        assert_eq!(b.height(Some("b")), MAX_HEIGHT);
        assert_eq!(b.height(Some("none")), 0);
        assert_eq!(b.height(None), 0);
        assert_eq!((MIN_HEIGHT, MAX_HEIGHT), (24, 160));
    }

    #[test]
    fn a_plugin_that_stops_takes_every_bar_and_a_closed_tab_takes_its_own() {
        let mut b = Bars::default();
        b.show("t1", "p", 40).unwrap();
        b.show("t2", "p", 40).unwrap();
        b.show("t3", "q", 40).unwrap();
        assert_eq!(b.clear_plugin("p"), ["t1", "t2"]);
        assert!(b.clear_plugin("p").is_empty());
        assert!(b.remove_tab("t3"));
        assert!(!b.remove_tab("t3"));
        assert!(b.is_empty());
    }

    #[test]
    fn a_plugin_keeps_two_live_bar_views_and_the_front_one_always() {
        let v = |t: &str, p: &str, s: u64| (t.to_string(), p.to_string(), s);
        let live = [
            v("a", "p", 1),
            v("b", "p", 5),
            v("c", "p", 3),
            v("d", "q", 0),
        ];
        assert_eq!(surplus(&live, Some("b")), ["a"]);
        // The front one stays even when it was seen longest ago.
        assert_eq!(surplus(&live, Some("a")), ["c"]);
        assert!(surplus(&live[..2], None).is_empty());
    }

    #[test]
    fn the_bars_give_way_so_that_twelve_plugin_views_stay_in_all() {
        let live: Vec<(String, String, u64)> = (0..8)
            .map(|i| (format!("t{i}"), format!("p{i}"), i as u64))
            .collect();
        // Eight bars and four other views fill the twelve: nothing goes.
        assert!(over_total(&live, Some("t7"), 4).is_empty());
        // Six background pages: two bars go, the oldest.
        assert_eq!(over_total(&live, Some("t7"), 6), ["t0", "t1"]);
        // The front one stays even when it was seen longest ago.
        assert_eq!(over_total(&live, Some("t0"), 6), ["t1", "t2"]);
        // Panels and background pages alone fill the twelve: only the front bar stays.
        let gone = over_total(&live, Some("t3"), 12);
        assert_eq!(gone.len(), 7);
        assert!(!gone.contains(&"t3".to_string()));
        // With no bar in front, none is kept past the total.
        assert_eq!(over_total(&live, None, 12).len(), 8);
        assert_eq!(MAX_VIEWS, 12);
    }
}
