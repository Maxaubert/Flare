//! What callers put on a tab besides its own marks: badges and a progress chip (plugins phase 1).
//!
//! Each piece is keyed by who set it (the tab's agent, later a plugin) and a short key, so setting the same key
//! replaces and clearing removes. A tab keeps every badge it was given (its tooltip lists them all) and draws only
//! a few: the progress first, then one badge per source, the agent's before any plugin's, newest first. How many
//! fit depends on the tab: none on an icon tab or under 110 px, one under 150 px, two from 150 px. Pure: the
//! window draws what [`Decor::drawn`] says, and the caller passes in nothing but data.

use std::collections::HashMap;

/// Keys one source may hold on one tab.
pub const MAX_KEYS_PER_TAB: usize = 4;
/// Keys one source may hold across all tabs.
pub const MAX_KEYS_TOTAL: usize = 200;
/// Chips a tab draws at most (a progress counts as one).
pub const MAX_DRAWN: usize = 2;
/// A named tab narrower than this draws no chip.
pub const ONE_CHIP_CSS: f64 = 110.0;
/// A named tab this wide or wider draws two.
pub const TWO_CHIPS_CSS: f64 = 150.0;

/// Who put a piece of decor on a tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// The tab's own agent (through `flare-mcp`). Cleared when its session changes, it leaves or the tab closes.
    Agent,
    /// A plugin, by id (the plugin phases).
    Plugin(String),
}

/// The ink a badge is drawn in. None is an agent state colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Ink,
    Accent,
    Muted,
}

impl Tone {
    pub fn parse(name: &str) -> Option<Tone> {
        match name {
            "ink" => Some(Tone::Ink),
            "accent" => Some(Tone::Accent),
            "muted" => Some(Tone::Muted),
            _ => None,
        }
    }

    /// The number the tab row carries (0 ink, 1 accent, 2 muted).
    pub fn code(self) -> i32 {
        match self {
            Tone::Ink => 0,
            Tone::Accent => 1,
            Tone::Muted => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub source: Source,
    pub key: String,
    /// A built-in icon name (`flare_ctl::decor::BADGE_ICONS`).
    pub icon: String,
    /// One clean line, or "" for none.
    pub tip: String,
    pub tone: Tone,
    /// When it was last set, in this tab's own count: the newest is drawn first.
    seq: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub source: Source,
    pub key: String,
    /// 0 to 1, or None for "under way, amount unknown" (drawn as a static dotted pill).
    pub value: Option<f32>,
    /// One clean line, or "" for none.
    pub label: String,
}

/// Why a badge was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The source holds [`MAX_KEYS_PER_TAB`] keys on this tab, or [`MAX_KEYS_TOTAL`] in all.
    TooManyKeys,
}

/// What a tab draws: the progress (Some(None) is the unknown amount) and the badges, in drawing order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn<'a> {
    pub progress: Option<Option<f32>>,
    pub badges: Vec<&'a Badge>,
}

/// The decor of one tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decor {
    badges: Vec<Badge>,
    progress: Option<Progress>,
    seq: u64,
}

impl Decor {
    pub fn is_empty(&self) -> bool {
        self.badges.is_empty() && self.progress.is_none()
    }

    /// Every badge, oldest set first.
    pub fn badges(&self) -> Vec<&Badge> {
        let mut v: Vec<&Badge> = self.badges.iter().collect();
        v.sort_by_key(|b| b.seq);
        v
    }

    pub fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref()
    }

    /// How many badge keys `source` holds here.
    pub fn keys_of(&self, source: &Source) -> usize {
        self.badges.iter().filter(|b| &b.source == source).count()
    }

    /// Sets the badge `(source, key)`, replacing one of the same key. A new key past the source's
    /// [`MAX_KEYS_PER_TAB`] is refused. True when anything a person sees changed.
    pub fn set_badge(
        &mut self,
        source: Source,
        key: &str,
        icon: &str,
        tip: &str,
        tone: Tone,
    ) -> Result<bool, Refused> {
        self.seq += 1;
        let seq = self.seq;
        if let Some(b) = self
            .badges
            .iter_mut()
            .find(|b| b.source == source && b.key == key)
        {
            let same = b.icon == icon && b.tip == tip && b.tone == tone;
            if !same {
                b.icon = icon.to_string();
                b.tip = tip.to_string();
                b.tone = tone;
                b.seq = seq;
            }
            return Ok(!same);
        }
        if self.keys_of(&source) >= MAX_KEYS_PER_TAB {
            return Err(Refused::TooManyKeys);
        }
        self.badges.push(Badge {
            source,
            key: key.to_string(),
            icon: icon.to_string(),
            tip: tip.to_string(),
            tone,
            seq,
        });
        Ok(true)
    }

    /// Removes the badge `(source, key)`. True when there was one.
    pub fn clear_badge(&mut self, source: &Source, key: &str) -> bool {
        let before = self.badges.len();
        self.badges
            .retain(|b| !(&b.source == source && b.key == key));
        self.badges.len() != before
    }

    /// Sets the tab's one progress (the last set wins, whoever set it). True when it changed.
    pub fn set_progress(
        &mut self,
        source: Source,
        key: &str,
        value: Option<f32>,
        label: &str,
    ) -> bool {
        let new = Progress {
            source,
            key: key.to_string(),
            value,
            label: label.to_string(),
        };
        if self.progress.as_ref() == Some(&new) {
            return false;
        }
        self.progress = Some(new);
        true
    }

    /// Removes the progress when it is `(source, key)`'s. True when it went.
    pub fn clear_progress(&mut self, source: &Source, key: &str) -> bool {
        if self
            .progress
            .as_ref()
            .is_some_and(|p| &p.source == source && p.key == key)
        {
            self.progress = None;
            return true;
        }
        false
    }

    /// Everything `source` put here goes. True when anything went.
    pub fn clear_source(&mut self, source: &Source) -> bool {
        let before = self.badges.len();
        self.badges.retain(|b| &b.source != source);
        let mut changed = self.badges.len() != before;
        if self.progress.as_ref().is_some_and(|p| &p.source == source) {
            self.progress = None;
            changed = true;
        }
        changed
    }

    /// What the tab draws given `room` chips: the progress first, then the newest badge of each source, the
    /// agent's (its reserved slot) before the plugins', the plugins newest first. Never more than [`MAX_DRAWN`].
    pub fn drawn(&self, room: usize) -> Drawn<'_> {
        let mut room = room.min(MAX_DRAWN);
        let mut out = Drawn::default();
        if room > 0
            && let Some(p) = &self.progress
        {
            out.progress = Some(p.value);
            room -= 1;
        }
        // The newest badge of each source.
        let mut newest: Vec<&Badge> = Vec::new();
        for b in &self.badges {
            match newest.iter_mut().find(|n| n.source == b.source) {
                Some(n) if n.seq < b.seq => *n = b,
                Some(_) => {}
                None => newest.push(b),
            }
        }
        newest.sort_by_key(|b| (b.source != Source::Agent, std::cmp::Reverse(b.seq)));
        out.badges = newest.into_iter().take(room).collect();
        out
    }

    /// One tooltip line per piece, each starting with the name of who set it (`name_of`): the progress first,
    /// then every badge, oldest first, drawn or not.
    pub fn tip_lines(&self, name_of: impl Fn(&Source) -> String) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(p) = &self.progress {
            let amount = p.value.map(|v| format!("{}%", (v * 100.0).round() as i32));
            let text = match (p.label.as_str(), amount) {
                ("", Some(a)) => a,
                ("", None) => "under way".to_string(),
                (l, Some(a)) => format!("{l} {a}"),
                (l, None) => l.to_string(),
            };
            lines.push(format!("{}: {text}", name_of(&p.source)));
        }
        for b in self.badges() {
            let text = if b.tip.is_empty() { &b.icon } else { &b.tip };
            lines.push(format!("{}: {text}", name_of(&b.source)));
        }
        lines
    }
}

/// How many chips a tab has room for: none on an icon or hidden tab or a named one under 110 css px, one under
/// 150, two from 150.
pub fn room_for(width_css: f64, named: bool) -> usize {
    if !named || width_css.is_nan() || width_css < ONE_CHIP_CSS {
        0
    } else if width_css < TWO_CHIPS_CSS {
        1
    } else {
        2
    }
}

/// The value a tab row carries for a progress: -1 none, -2 an unknown amount, else 0 to 1.
pub fn progress_code(p: Option<Option<f32>>) -> f32 {
    match p {
        None => -1.0,
        Some(None) => -2.0,
        Some(Some(v)) if v.is_nan() => 0.0,
        Some(Some(v)) => v.clamp(0.0, 1.0),
    }
}

/// The decor of every tab, with the per-source cap across tabs.
#[derive(Clone, Debug, Default)]
pub struct Decors {
    tabs: HashMap<String, Decor>,
}

impl Decors {
    pub fn get(&self, tab: &str) -> Option<&Decor> {
        self.tabs.get(tab)
    }

    /// The badge keys `source` holds on every tab together.
    pub fn keys_total(&self, source: &Source) -> usize {
        self.tabs.values().map(|d| d.keys_of(source)).sum()
    }

    /// [`Decor::set_badge`] on `tab`, with the [`MAX_KEYS_TOTAL`] cap for a new key.
    pub fn set_badge(
        &mut self,
        tab: &str,
        source: Source,
        key: &str,
        icon: &str,
        tip: &str,
        tone: Tone,
    ) -> Result<bool, Refused> {
        let exists = self
            .tabs
            .get(tab)
            .is_some_and(|d| d.badges.iter().any(|b| b.source == source && b.key == key));
        if !exists && self.keys_total(&source) >= MAX_KEYS_TOTAL {
            return Err(Refused::TooManyKeys);
        }
        self.tabs
            .entry(tab.to_string())
            .or_default()
            .set_badge(source, key, icon, tip, tone)
    }

    pub fn clear_badge(&mut self, tab: &str, source: &Source, key: &str) -> bool {
        self.with(tab, |d| d.clear_badge(source, key))
    }

    pub fn set_progress(
        &mut self,
        tab: &str,
        source: Source,
        key: &str,
        value: Option<f32>,
        label: &str,
    ) -> bool {
        self.tabs
            .entry(tab.to_string())
            .or_default()
            .set_progress(source, key, value, label)
    }

    pub fn clear_progress(&mut self, tab: &str, source: &Source, key: &str) -> bool {
        self.with(tab, |d| d.clear_progress(source, key))
    }

    /// Everything `source` put on `tab` goes.
    pub fn clear_source(&mut self, tab: &str, source: &Source) -> bool {
        self.with(tab, |d| d.clear_source(source))
    }

    /// Everything `source` put on any tab goes (a plugin turned off, or its background page ended). True when
    /// anything went.
    pub fn clear_source_everywhere(&mut self, source: &Source) -> bool {
        let mut changed = false;
        for d in self.tabs.values_mut() {
            changed |= d.clear_source(source);
        }
        self.tabs.retain(|_, d| !d.is_empty());
        changed
    }

    /// The tabs that hold something of `source`, sorted (tests and the probe).
    pub fn tabs_of(&self, source: &Source) -> Vec<String> {
        let mut v: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, d)| {
                d.badges.iter().any(|b| &b.source == source)
                    || d.progress.as_ref().is_some_and(|p| &p.source == source)
            })
            .map(|(t, _)| t.clone())
            .collect();
        v.sort();
        v
    }

    /// The tab closed: all of its decor goes.
    pub fn remove_tab(&mut self, tab: &str) -> bool {
        self.tabs.remove(tab).is_some_and(|d| !d.is_empty())
    }

    /// Runs `f` on the tab's decor and drops the entry once it is empty.
    fn with(&mut self, tab: &str, f: impl FnOnce(&mut Decor) -> bool) -> bool {
        let Some(d) = self.tabs.get_mut(tab) else {
            return false;
        };
        let changed = f(d);
        if d.is_empty() {
            self.tabs.remove(tab);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(id: &str) -> Source {
        Source::Plugin(id.to_string())
    }

    fn icons<'a>(d: &'a Drawn<'a>) -> Vec<&'a str> {
        d.badges.iter().map(|b| b.icon.as_str()).collect()
    }

    #[test]
    fn a_source_cleared_everywhere_leaves_every_tab_and_the_others_keep_theirs() {
        let mut all = Decors::default();
        for tab in ["a", "b", "c"] {
            all.set_badge(tab, plugin("todo"), "k", "star", "", Tone::Ink)
                .unwrap();
        }
        all.set_progress("b", plugin("todo"), "p", Some(0.5), "");
        all.set_badge("c", Source::Agent, "call-me", "phone", "", Tone::Ink)
            .unwrap();
        assert_eq!(all.tabs_of(&plugin("todo")), ["a", "b", "c"]);
        assert!(all.clear_source_everywhere(&plugin("todo")));
        assert!(all.tabs_of(&plugin("todo")).is_empty());
        assert!(
            all.get("a").is_none() && all.get("b").is_none(),
            "empty tabs go"
        );
        assert_eq!(all.get("c").unwrap().badges().len(), 1, "the agent's stays");
        assert!(!all.clear_source_everywhere(&plugin("todo")));
    }

    /// Two plugins on one tab are drawn once each: a plugin with several badges there shows its newest only.
    #[test]
    fn each_plugin_draws_at_most_one_badge_on_a_tab() {
        let mut d = Decor::default();
        d.set_badge(plugin("a"), "one", "star", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("a"), "two", "bell", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("a"), "three", "moon", "", Tone::Ink)
            .unwrap();
        let drawn = d.drawn(2);
        assert_eq!(icons(&drawn), ["moon"]);
        d.set_badge(plugin("b"), "x", "globe", "", Tone::Ink)
            .unwrap();
        let drawn = d.drawn(2);
        assert_eq!(icons(&drawn), ["globe", "moon"]);
    }

    #[test]
    fn the_same_key_replaces_and_a_clear_removes() {
        let mut d = Decor::default();
        assert_eq!(
            d.set_badge(
                Source::Agent,
                "call-me",
                "phone",
                "Call-me is on",
                Tone::Ink
            ),
            Ok(true)
        );
        // The same content again changes nothing a person sees.
        assert_eq!(
            d.set_badge(
                Source::Agent,
                "call-me",
                "phone",
                "Call-me is on",
                Tone::Ink
            ),
            Ok(false)
        );
        assert_eq!(
            d.set_badge(Source::Agent, "call-me", "bell", "", Tone::Accent),
            Ok(true)
        );
        assert_eq!(d.badges().len(), 1);
        assert_eq!(d.badges()[0].icon, "bell");
        assert_eq!(d.badges()[0].tone, Tone::Accent);
        // A key of another source is another badge.
        assert_eq!(
            d.set_badge(plugin("todo"), "call-me", "star", "", Tone::Ink),
            Ok(true)
        );
        assert_eq!(d.badges().len(), 2);
        assert!(d.clear_badge(&Source::Agent, "call-me"));
        assert!(!d.clear_badge(&Source::Agent, "call-me"));
        assert_eq!(d.badges().len(), 1);
        assert_eq!(d.badges()[0].source, plugin("todo"));
    }

    #[test]
    fn a_source_keeps_at_most_four_keys_a_tab() {
        let mut d = Decor::default();
        for k in ["a", "b", "c", "d"] {
            assert_eq!(
                d.set_badge(Source::Agent, k, "star", "", Tone::Ink),
                Ok(true)
            );
        }
        assert_eq!(
            d.set_badge(Source::Agent, "e", "star", "", Tone::Ink),
            Err(Refused::TooManyKeys)
        );
        // Replacing a held key is still fine, and another source has its own four.
        assert_eq!(
            d.set_badge(Source::Agent, "a", "moon", "", Tone::Ink),
            Ok(true)
        );
        assert_eq!(
            d.set_badge(plugin("p"), "e", "star", "", Tone::Ink),
            Ok(true)
        );
        assert_eq!(d.keys_of(&Source::Agent), MAX_KEYS_PER_TAB);
    }

    #[test]
    fn a_source_keeps_at_most_two_hundred_keys_in_all() {
        let mut all = Decors::default();
        let p = plugin("busy");
        for i in 0..MAX_KEYS_TOTAL {
            let tab = format!("t{}", i / MAX_KEYS_PER_TAB);
            let key = format!("k{}", i % MAX_KEYS_PER_TAB);
            assert_eq!(
                all.set_badge(&tab, p.clone(), &key, "star", "", Tone::Ink),
                Ok(true)
            );
        }
        assert_eq!(all.keys_total(&p), MAX_KEYS_TOTAL);
        assert_eq!(
            all.set_badge("fresh", p.clone(), "k0", "star", "", Tone::Ink),
            Err(Refused::TooManyKeys)
        );
        // A held key can still change, and the agent is not counted with the plugin.
        assert_eq!(
            all.set_badge("t0", p.clone(), "k0", "moon", "", Tone::Ink),
            Ok(true)
        );
        assert_eq!(
            all.set_badge("fresh", Source::Agent, "k0", "star", "", Tone::Ink),
            Ok(true)
        );
    }

    #[test]
    fn progress_is_one_per_tab_and_cleared_only_by_its_own_key() {
        let mut d = Decor::default();
        assert!(d.set_progress(Source::Agent, "progress", Some(0.25), "Build"));
        assert!(!d.set_progress(Source::Agent, "progress", Some(0.25), "Build"));
        assert!(d.set_progress(Source::Agent, "progress", None, ""));
        // Another key of the same source, or another source, takes the one slot (the last set is drawn).
        assert!(d.set_progress(plugin("ci"), "run", Some(0.5), ""));
        assert!(!d.clear_progress(&Source::Agent, "progress"));
        assert!(!d.clear_progress(&plugin("ci"), "other"));
        assert!(d.clear_progress(&plugin("ci"), "run"));
        assert!(d.progress().is_none());
        assert!(d.is_empty());
    }

    #[test]
    fn clearing_a_source_takes_its_badges_and_its_progress_and_nothing_else() {
        let mut d = Decor::default();
        d.set_badge(Source::Agent, "a", "phone", "", Tone::Ink)
            .unwrap();
        d.set_badge(Source::Agent, "b", "bell", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("p"), "a", "globe", "", Tone::Ink)
            .unwrap();
        d.set_progress(Source::Agent, "progress", Some(0.5), "");
        assert!(d.clear_source(&Source::Agent));
        assert!(!d.clear_source(&Source::Agent));
        assert_eq!(d.badges().len(), 1);
        assert!(d.progress().is_none());
        // A plugin's progress stays when the agent's source is cleared.
        d.set_progress(plugin("p"), "run", None, "");
        assert!(!d.clear_source(&Source::Agent));
        assert!(d.progress().is_some());
    }

    #[test]
    fn the_room_follows_the_tab_width_and_an_icon_tab_has_none() {
        assert_eq!(room_for(109.9, true), 0);
        assert_eq!(room_for(110.0, true), 1);
        assert_eq!(room_for(149.0, true), 1);
        assert_eq!(room_for(150.0, true), 2);
        assert_eq!(room_for(400.0, true), 2);
        assert_eq!(room_for(400.0, false), 0);
        assert_eq!(room_for(f64::NAN, true), 0);
    }

    #[test]
    fn a_tab_draws_the_progress_then_the_agents_badge_then_the_newest_plugins() {
        let mut d = Decor::default();
        d.set_badge(plugin("old"), "x", "moon", "", Tone::Ink)
            .unwrap();
        d.set_badge(Source::Agent, "a", "phone", "", Tone::Ink)
            .unwrap();
        d.set_badge(Source::Agent, "b", "bell", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("new"), "y", "globe", "", Tone::Ink)
            .unwrap();
        // Two chips: the agent's newest (its one slot), then the newest plugin's.
        let two = d.drawn(2);
        assert_eq!(two.progress, None);
        assert_eq!(icons(&two), ["bell", "globe"]);
        let one = d.drawn(1);
        assert_eq!(icons(&one), ["bell"]);
        assert!(d.drawn(0).badges.is_empty());
        // Room is never more than two.
        assert_eq!(d.drawn(9).badges.len(), MAX_DRAWN);
        // A progress takes the first chip.
        d.set_progress(Source::Agent, "progress", Some(0.4), "");
        let two = d.drawn(2);
        assert_eq!(two.progress, Some(Some(0.4)));
        assert_eq!(icons(&two), ["bell"]);
        let one = d.drawn(1);
        assert_eq!(one.progress, Some(Some(0.4)));
        assert!(one.badges.is_empty());
        assert_eq!(d.drawn(0), Drawn::default());
    }

    #[test]
    fn each_source_draws_one_badge_and_a_replaced_key_is_the_newest() {
        let mut d = Decor::default();
        d.set_badge(plugin("p"), "a", "star", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("p"), "b", "lock", "", Tone::Ink)
            .unwrap();
        d.set_badge(plugin("q"), "a", "eye", "", Tone::Ink).unwrap();
        assert_eq!(icons(&d.drawn(2)), ["eye", "lock"]);
        // p sets its first key again: p's newest is now star, and p is the newest source.
        d.set_badge(plugin("p"), "a", "star", "again", Tone::Ink)
            .unwrap();
        assert_eq!(icons(&d.drawn(2)), ["star", "eye"]);
    }

    #[test]
    fn the_tooltip_lists_everything_with_who_set_it() {
        let mut d = Decor::default();
        d.set_badge(
            Source::Agent,
            "call-me",
            "phone",
            "Call-me is on",
            Tone::Ink,
        )
        .unwrap();
        d.set_badge(Source::Agent, "x", "star", "", Tone::Ink)
            .unwrap();
        d.set_progress(Source::Agent, "progress", Some(0.456), "Review");
        let name = |s: &Source| match s {
            Source::Agent => "Claude".to_string(),
            Source::Plugin(id) => id.clone(),
        };
        assert_eq!(
            d.tip_lines(name),
            [
                "Claude: Review 46%",
                "Claude: Call-me is on",
                "Claude: star"
            ]
        );
        d.set_progress(Source::Agent, "progress", None, "");
        assert_eq!(d.tip_lines(name)[0], "Claude: under way");
        d.set_progress(Source::Agent, "progress", Some(1.0), "");
        assert_eq!(d.tip_lines(name)[0], "Claude: 100%");
        d.set_progress(Source::Agent, "progress", None, "Waiting");
        assert_eq!(d.tip_lines(name)[0], "Claude: Waiting");
    }

    #[test]
    fn a_progress_is_carried_as_one_number() {
        assert_eq!(progress_code(None), -1.0);
        assert_eq!(progress_code(Some(None)), -2.0);
        assert_eq!(progress_code(Some(Some(0.5))), 0.5);
        assert_eq!(progress_code(Some(Some(3.0))), 1.0);
        assert_eq!(progress_code(Some(Some(-1.0))), 0.0);
        assert_eq!(progress_code(Some(Some(f32::NAN))), 0.0);
    }

    #[test]
    fn a_tab_with_nothing_left_is_dropped_and_a_closed_tab_takes_its_decor() {
        let mut all = Decors::default();
        all.set_badge("t", Source::Agent, "a", "phone", "", Tone::Ink)
            .unwrap();
        assert!(all.clear_badge("t", &Source::Agent, "a"));
        assert!(all.get("t").is_none());
        assert!(!all.clear_badge("nope", &Source::Agent, "a"));
        all.set_progress("t", Source::Agent, "progress", None, "");
        assert!(all.clear_source("t", &Source::Agent));
        assert!(all.get("t").is_none());
        all.set_progress("t", Source::Agent, "progress", None, "");
        assert!(all.remove_tab("t"));
        assert!(!all.remove_tab("t"));
    }
}
