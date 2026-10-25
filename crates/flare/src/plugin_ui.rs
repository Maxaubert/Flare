//! Plugin sidebar sections and actions (plugins phase 4, design 5.3, 5.4 and 13), the pure part: what the plugins
//! set (`sidebar.set` items, `actions.set` state), the order the person gave the sections, and what each surface
//! draws from them. No Slint and no window: `appctl` keeps one [`PluginUi`] and turns its answers into rows.
//!
//! - A section is declared in a plugin's `contributes.sidebar` and shown while the plugin is on. Flare draws it in
//!   the sidebar's look under the plugin's name; at most [`SECTIONS_MAX`] are drawn in all, in the person's order
//!   (moved up and down from the section's header menu, kept in the preferences), then the rest in the order the
//!   plugins were found.
//! - An action is declared in `contributes.actions` with its places. `actions.set` changes its state on one tab, or
//!   on every tab when no tab is named; a tab's own state wins over the plugin-wide one, field by field.
//! - Everything a plugin set goes when it stops ([`PluginUi::clear_plugin`]); a closed tab takes its action state.

use std::collections::{HashMap, HashSet};

/// Items one section holds at most.
pub const MAX_ITEMS: usize = 50;
/// The longest `text` and `sub` of an item, in characters.
pub const ITEM_TEXT_MAX: usize = 120;
/// The longest item id.
pub const ITEM_ID_MAX: usize = 64;
/// The longest title an action may be given by `actions.set` (the manifest's title rule).
pub const ACTION_TITLE_MAX: usize = flare_mods::manifest::MAX_TITLE_CHARS;
/// Plugin sections drawn at most, all plugins together (design 13).
pub const SECTIONS_MAX: usize = 6;
/// Top-bar buttons drawn at most, all plugins together (design 5.4).
pub const TOP_BAR_MAX: usize = 3;
/// Hover buttons a tab draws at most (one per plugin, design 5.4).
pub const HOVER_MAX: usize = 2;
/// `sidebar.set` calls a second for one section (design 13).
pub const SIDEBAR_RATE_PER_KEY: f64 = 5.0;
/// `actions.set` calls a second for one plugin (design 13).
pub const ACTIONS_RATE: f64 = 20.0;
/// The preference that holds the person's order of the sections: their ids, one per line.
pub const ORDER_PREF: &str = "plugins.sidebar.order";
/// The preference that holds the sections the person folded: their ids, one per line.
pub const FOLDED_PREF: &str = "plugins.sidebar.folded";

/// A section's id: `<plugin>/<key>` (both are `a-z`, `0-9` and `-`, so the slash is never part of either).
pub fn section_id(plugin: &str, key: &str) -> String {
    format!("{plugin}/{key}")
}

/// The plugin and key of a section id.
pub fn split_section(sid: &str) -> Option<(&str, &str)> {
    sid.split_once('/')
}

/// One item of a section, as `sidebar.set` gave it (checked on the channel's worker).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub text: String,
    /// The second line; "" for none.
    pub sub: String,
    /// A built-in badge icon or one of the plugin's own; "" for none.
    pub icon: String,
    /// `Some` draws a checkbox.
    pub checked: Option<bool>,
    /// A tab the plugin may see: a click on the item brings it to the front.
    pub tab: Option<String>,
}

/// The state `actions.set` gives an action: each field it names replaces the one before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionState {
    pub enabled: Option<bool>,
    pub checked: Option<bool>,
    pub title: Option<String>,
}

impl ActionState {
    /// `self` with what `over` names laid on top.
    fn with(&self, over: &ActionState) -> ActionState {
        ActionState {
            enabled: over.enabled.or(self.enabled),
            checked: over.checked.or(self.checked),
            title: over.title.clone().or_else(|| self.title.clone()),
        }
    }
}

/// A section a plugin that is on declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionDecl {
    pub plugin: String,
    pub plugin_name: String,
    pub key: String,
    pub title: String,
}

impl SectionDecl {
    pub fn sid(&self) -> String {
        section_id(&self.plugin, &self.key)
    }
}

/// What the plugins set, by section and action.
#[derive(Debug, Default)]
pub struct PluginUi {
    /// By section id.
    lists: HashMap<String, Vec<Item>>,
    /// By (plugin, key): the state on every tab.
    wide: HashMap<(String, String), ActionState>,
    /// By (plugin, key, tab).
    per_tab: HashMap<(String, String, String), ActionState>,
}

impl PluginUi {
    /// The section's items, replaced. True when they changed.
    pub fn set_items(&mut self, plugin: &str, key: &str, items: Vec<Item>) -> bool {
        let sid = section_id(plugin, key);
        if self.lists.get(&sid) == Some(&items) {
            return false;
        }
        self.lists.insert(sid, items);
        true
    }

    pub fn items(&self, sid: &str) -> &[Item] {
        self.lists.get(sid).map_or(&[], Vec::as_slice)
    }

    /// The item, if the section holds it.
    pub fn item(&self, sid: &str, id: &str) -> Option<&Item> {
        self.items(sid).iter().find(|i| i.id == id)
    }

    /// The person ticked or unticked the item's checkbox: it shows the new state at once (the plugin hears of it and
    /// may set the list again). The new state, or None when the item has no checkbox.
    pub fn toggle(&mut self, sid: &str, id: &str) -> Option<bool> {
        let item = self.lists.get_mut(sid)?.iter_mut().find(|i| i.id == id)?;
        let now = !item.checked?;
        item.checked = Some(now);
        Some(now)
    }

    /// The action's state on `tab`, or on every tab for None, with the fields `state` names replaced. True when what
    /// it says changed.
    pub fn set_action(
        &mut self,
        plugin: &str,
        key: &str,
        tab: Option<&str>,
        state: &ActionState,
    ) -> bool {
        let slot = match tab {
            Some(t) => self
                .per_tab
                .entry((plugin.to_string(), key.to_string(), t.to_string()))
                .or_default(),
            None => self
                .wide
                .entry((plugin.to_string(), key.to_string()))
                .or_default(),
        };
        let next = slot.with(state);
        if *slot == next {
            return false;
        }
        *slot = next;
        true
    }

    /// What the action says on `tab` (None: where no tab is meant, the top bar): the tab's own state over the
    /// plugin-wide one.
    pub fn action(&self, plugin: &str, key: &str, tab: Option<&str>) -> ActionState {
        let wide = self
            .wide
            .get(&(plugin.to_string(), key.to_string()))
            .cloned()
            .unwrap_or_default();
        match tab.and_then(|t| {
            self.per_tab
                .get(&(plugin.to_string(), key.to_string(), t.to_string()))
        }) {
            Some(own) => wide.with(own),
            None => wide,
        }
    }

    /// Everything the plugin set goes (it stopped). True when anything went.
    pub fn clear_plugin(&mut self, plugin: &str) -> bool {
        let before = self.lists.len() + self.wide.len() + self.per_tab.len();
        self.lists
            .retain(|sid, _| split_section(sid).is_none_or(|(p, _)| p != plugin));
        self.wide.retain(|(p, _), _| p != plugin);
        self.per_tab.retain(|(p, _, _), _| p != plugin);
        before != self.lists.len() + self.wide.len() + self.per_tab.len()
    }

    /// The tab closed: the action state set for it goes. True when anything went.
    pub fn remove_tab(&mut self, tab: &str) -> bool {
        let before = self.per_tab.len();
        self.per_tab.retain(|(_, _, t), _| t != tab);
        before != self.per_tab.len()
    }
}

/// Section ids, one per line, as the preferences keep them; anything that is not one is dropped.
pub fn parse_ids(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        let ok = split_section(line).is_some_and(|(p, k)| {
            flare_mods::manifest::valid_id(p) && flare_ctl::decor::valid_key(k)
        });
        if ok && !out.iter().any(|o| o == line) {
            out.push(line.to_string());
        }
    }
    out
}

pub fn join_ids(ids: &[String]) -> String {
    ids.join("\n")
}

/// The sections drawn, in order: those the person ordered first, in that order, then the rest as the plugins
/// declare them; at most [`SECTIONS_MAX`].
pub fn ordered<'a>(decls: &'a [SectionDecl], order: &[String]) -> Vec<&'a SectionDecl> {
    let mut out: Vec<&SectionDecl> = Vec::new();
    for sid in order {
        if let Some(d) = decls.iter().find(|d| d.sid() == *sid) {
            out.push(d);
        }
    }
    for d in decls {
        if !out.iter().any(|o| o.sid() == d.sid()) {
            out.push(d);
        }
    }
    out.truncate(SECTIONS_MAX);
    out
}

/// The order after the person moved `sid` one place up (or down) among the sections drawn: the whole new order of
/// what is drawn, followed by the ids the old order held that are not drawn now (a plugin that is off keeps its
/// place for when it is on again). None when it cannot move that way.
pub fn moved(drawn: &[String], order: &[String], sid: &str, up: bool) -> Option<Vec<String>> {
    let i = drawn.iter().position(|d| d == sid)?;
    let j = if up {
        i.checked_sub(1)?
    } else {
        (i + 1 < drawn.len()).then_some(i + 1)?
    };
    let mut next: Vec<String> = drawn.to_vec();
    next.swap(i, j);
    for o in order {
        if !next.contains(o) {
            next.push(o.clone());
        }
    }
    Some(next)
}

/// One row of the sections as the sidebar and the Plugins dropdown draw them.
#[derive(Clone, Debug, PartialEq)]
pub struct RowOut {
    /// 0 a section's header, 1 an item, 2 the note of an empty section.
    pub kind: i32,
    pub sid: String,
    /// The item's id ("" for a header and a note).
    pub id: String,
    /// A header: the plugin's name. An item: its text. A note: its words.
    pub text: String,
    /// A header: the section's title when it is not the plugin's name. An item: its second line.
    pub sub: String,
    /// The item's glyph path ("" for none).
    pub icon: String,
    /// -1 no checkbox, 0 unticked, 1 ticked.
    pub check: i32,
    /// A click brings a tab to the front.
    pub jump: bool,
    /// A header: the section is folded (its items are not listed).
    pub folded: bool,
    /// A header: it can move up, and down.
    pub can_up: bool,
    pub can_down: bool,
}

/// The words an empty section shows.
pub const EMPTY_NOTE: &str = "Nothing here yet";

/// The rows of the sections drawn, in order: each header, then (unless it is folded) its items or the empty note.
/// `glyph(plugin, icon)` is the path of an item's icon ("" for none or one the plugin does not have).
pub fn rows(
    drawn: &[&SectionDecl],
    ui: &PluginUi,
    folded: &HashSet<String>,
    glyph: &dyn Fn(&str, &str) -> String,
) -> Vec<RowOut> {
    let mut out = Vec::new();
    let n = drawn.len();
    for (i, d) in drawn.iter().enumerate() {
        let sid = d.sid();
        let fold = folded.contains(&sid);
        out.push(RowOut {
            kind: 0,
            sid: sid.clone(),
            id: String::new(),
            text: d.plugin_name.clone(),
            sub: if d.title == d.plugin_name {
                String::new()
            } else {
                d.title.clone()
            },
            icon: String::new(),
            check: -1,
            jump: false,
            folded: fold,
            can_up: i > 0,
            can_down: i + 1 < n,
        });
        if fold {
            continue;
        }
        let items = ui.items(&sid);
        if items.is_empty() {
            out.push(RowOut {
                kind: 2,
                sid: sid.clone(),
                id: String::new(),
                text: EMPTY_NOTE.into(),
                sub: String::new(),
                icon: String::new(),
                check: -1,
                jump: false,
                folded: false,
                can_up: false,
                can_down: false,
            });
        }
        for it in items {
            out.push(RowOut {
                kind: 1,
                sid: sid.clone(),
                id: it.id.clone(),
                text: it.text.clone(),
                sub: it.sub.clone(),
                icon: if it.icon.is_empty() {
                    String::new()
                } else {
                    glyph(&d.plugin, &it.icon)
                },
                check: it.checked.map_or(-1, i32::from),
                jump: it.tab.is_some(),
                folded: false,
                can_up: false,
                can_down: false,
            });
        }
    }
    out
}

/// One of a plugin's actions in one place, as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub plugin: String,
    pub key: String,
    /// The glyph's path ("" when the action has no icon: then its title's first letter is drawn).
    pub cmd: String,
    pub letter: String,
    /// The title the action wears now.
    pub title: String,
    /// The tooltip and the menu's words: the plugin's name, then the title.
    pub tip: String,
    pub checked: bool,
    pub enabled: bool,
}

/// What a plugin's action places need to know of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionDecl {
    pub plugin: String,
    pub plugin_name: String,
    pub key: String,
    pub title: String,
    pub icon: Option<String>,
    pub places: Vec<flare_mods::manifest::Place>,
}

fn button(a: &ActionDecl, s: &ActionState, glyph: &dyn Fn(&str, &str) -> String) -> Button {
    let title = s.title.clone().unwrap_or_else(|| a.title.clone());
    let letter = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    Button {
        plugin: a.plugin.clone(),
        key: a.key.clone(),
        cmd: a
            .icon
            .as_deref()
            .map(|i| glyph(&a.plugin, i))
            .unwrap_or_default(),
        letter,
        tip: format!("{}: {title}", a.plugin_name),
        title,
        checked: s.checked == Some(true),
        enabled: s.enabled != Some(false),
    }
}

/// The actions of `place` on `tab` (None: no tab, the top bar), plugin by plugin in the order they were found;
/// at most `cap` in all.
pub fn buttons(
    decls: &[ActionDecl],
    ui: &PluginUi,
    place: flare_mods::manifest::Place,
    tab: Option<&str>,
    cap: usize,
    glyph: &dyn Fn(&str, &str) -> String,
) -> Vec<Button> {
    decls
        .iter()
        .filter(|a| a.places.contains(&place))
        .take(cap)
        .map(|a| button(a, &ui.action(&a.plugin, &a.key, tab), glyph))
        .collect()
}

/// The tab menu's plugin groups: for each plugin with a `tab-menu` action, its name and its items on `tab`.
pub fn menu_groups(
    decls: &[ActionDecl],
    ui: &PluginUi,
    tab: &str,
    glyph: &dyn Fn(&str, &str) -> String,
) -> Vec<(String, Vec<Button>)> {
    let mut out: Vec<(String, Vec<Button>)> = Vec::new();
    for a in decls
        .iter()
        .filter(|a| a.places.contains(&flare_mods::manifest::Place::TabMenu))
    {
        let b = button(a, &ui.action(&a.plugin, &a.key, Some(tab)), glyph);
        match out.last_mut() {
            Some((_, list)) if list.first().is_some_and(|f| f.plugin == a.plugin) => list.push(b),
            _ => out.push((a.plugin_name.clone(), vec![b])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_mods::manifest::Place;

    fn item(id: &str) -> Item {
        Item {
            id: id.into(),
            text: format!("do {id}"),
            sub: String::new(),
            icon: String::new(),
            checked: None,
            tab: None,
        }
    }

    fn decl(plugin: &str, key: &str) -> SectionDecl {
        SectionDecl {
            plugin: plugin.into(),
            plugin_name: format!("Plugin {plugin}"),
            key: key.into(),
            title: "To-dos".into(),
        }
    }

    fn no_glyph(_: &str, i: &str) -> String {
        format!("path:{i}")
    }

    #[test]
    fn setting_the_same_items_again_is_no_change_and_a_tick_flips_only_a_checkbox() {
        let mut ui = PluginUi::default();
        let mut a = item("a");
        a.checked = Some(false);
        assert!(ui.set_items("p", "k", vec![a.clone(), item("b")]));
        assert!(!ui.set_items("p", "k", vec![a, item("b")]));
        assert_eq!(ui.toggle("p/k", "a"), Some(true));
        assert_eq!(ui.item("p/k", "a").unwrap().checked, Some(true));
        assert_eq!(ui.toggle("p/k", "a"), Some(false));
        // No checkbox, no item, no section: nothing to tick.
        assert_eq!(ui.toggle("p/k", "b"), None);
        assert_eq!(ui.toggle("p/k", "zz"), None);
        assert_eq!(ui.toggle("q/k", "a"), None);
    }

    #[test]
    fn a_tabs_own_action_state_wins_field_by_field_and_goes_with_the_tab() {
        let mut ui = PluginUi::default();
        let wide = ActionState {
            enabled: Some(true),
            checked: Some(false),
            title: Some("Call me".into()),
        };
        assert!(ui.set_action("p", "call", None, &wide));
        assert!(!ui.set_action("p", "call", None, &wide));
        let own = ActionState {
            checked: Some(true),
            ..ActionState::default()
        };
        assert!(ui.set_action("p", "call", Some("t1"), &own));
        let on_t1 = ui.action("p", "call", Some("t1"));
        assert_eq!(on_t1.checked, Some(true));
        assert_eq!(on_t1.title.as_deref(), Some("Call me"));
        assert_eq!(ui.action("p", "call", Some("t2")).checked, Some(false));
        assert_eq!(ui.action("p", "call", None), wide);
        // A field left out keeps what it was.
        ui.set_action(
            "p",
            "call",
            Some("t1"),
            &ActionState {
                enabled: Some(false),
                ..ActionState::default()
            },
        );
        assert_eq!(ui.action("p", "call", Some("t1")).checked, Some(true));
        assert!(ui.remove_tab("t1"));
        assert!(!ui.remove_tab("t1"));
        assert_eq!(ui.action("p", "call", Some("t1")), wide);
    }

    #[test]
    fn a_plugin_that_stops_takes_its_lists_and_states_and_nothing_else() {
        let mut ui = PluginUi::default();
        ui.set_items("p", "k", vec![item("a")]);
        ui.set_items("q", "k", vec![item("b")]);
        ui.set_action(
            "p",
            "x",
            Some("t"),
            &ActionState {
                checked: Some(true),
                ..ActionState::default()
            },
        );
        assert!(ui.clear_plugin("p"));
        assert!(!ui.clear_plugin("p"));
        assert!(ui.items("p/k").is_empty());
        assert_eq!(ui.items("q/k").len(), 1);
        assert_eq!(ui.action("p", "x", Some("t")), ActionState::default());
    }

    #[test]
    fn the_persons_order_comes_first_then_the_rest_and_six_at_most() {
        let decls: Vec<SectionDecl> = (0..8).map(|i| decl(&format!("p{i}"), "k")).collect();
        let order = vec!["p5/k".to_string(), "gone/k".to_string(), "p2/k".to_string()];
        let drawn: Vec<String> = ordered(&decls, &order).iter().map(|d| d.sid()).collect();
        assert_eq!(drawn, ["p5/k", "p2/k", "p0/k", "p1/k", "p3/k", "p4/k"]);
        assert_eq!(drawn.len(), SECTIONS_MAX);
    }

    #[test]
    fn moving_a_section_swaps_it_with_its_neighbour_and_keeps_the_absent_ones() {
        let drawn = vec!["a/k".to_string(), "b/k".to_string(), "c/k".to_string()];
        let order = vec!["off/k".to_string()];
        assert_eq!(
            moved(&drawn, &order, "b/k", true).unwrap(),
            ["b/k", "a/k", "c/k", "off/k"]
        );
        assert_eq!(
            moved(&drawn, &order, "b/k", false).unwrap(),
            ["a/k", "c/k", "b/k", "off/k"]
        );
        assert_eq!(moved(&drawn, &order, "a/k", true), None);
        assert_eq!(moved(&drawn, &order, "c/k", false), None);
        assert_eq!(moved(&drawn, &order, "x/k", false), None);
    }

    #[test]
    fn stored_ids_are_read_back_and_junk_is_dropped() {
        let ids = parse_ids("todo/todos\n\nnot an id\ntodo/todos\nAB/x\nother/two-1\n");
        assert_eq!(ids, ["todo/todos", "other/two-1"]);
        assert_eq!(parse_ids(&join_ids(&ids)), ids);
    }

    #[test]
    fn the_rows_are_each_header_then_its_items_or_a_note_and_a_folded_one_has_only_its_header() {
        let decls = vec![decl("pa", "k"), decl("pb", "k"), decl("pc", "k")];
        let drawn = ordered(&decls, &[]);
        let mut ui = PluginUi::default();
        let mut a = item("a");
        a.checked = Some(true);
        a.tab = Some("t1".into());
        a.icon = "star".into();
        a.sub = "second".into();
        ui.set_items("pa", "k", vec![a, item("b")]);
        ui.set_items("pc", "k", vec![item("c")]);
        let folded: HashSet<String> = ["pc/k".to_string()].into();
        let r = rows(&drawn, &ui, &folded, &no_glyph);
        let kinds: Vec<(i32, &str)> = r.iter().map(|r| (r.kind, r.text.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (0, "Plugin pa"),
                (1, "do a"),
                (1, "do b"),
                (0, "Plugin pb"),
                (2, EMPTY_NOTE),
                (0, "Plugin pc"),
            ]
        );
        assert_eq!(r[0].sub, "To-dos");
        assert!(!r[0].can_up && r[0].can_down);
        assert!(r[5].folded && r[5].can_up && !r[5].can_down);
        assert_eq!(
            (r[1].check, r[1].jump, r[1].icon.as_str(), r[1].sub.as_str()),
            (1, true, "path:star", "second")
        );
        assert_eq!((r[2].check, r[2].jump, r[2].icon.as_str()), (-1, false, ""));
        // A section titled as its plugin is shows the name once.
        let mut same = decl("pd", "k");
        same.title = same.plugin_name.clone();
        let one = [&same];
        assert_eq!(rows(&one, &ui, &HashSet::new(), &no_glyph)[0].sub, "");
    }

    fn action(plugin: &str, key: &str, places: &[Place]) -> ActionDecl {
        ActionDecl {
            plugin: plugin.into(),
            plugin_name: format!("Plugin {plugin}"),
            key: key.into(),
            title: format!("do {key}"),
            icon: (key != "bare").then(|| "plus".to_string()),
            places: places.to_vec(),
        }
    }

    #[test]
    fn buttons_wear_their_state_their_plugins_name_and_a_letter_without_an_icon() {
        let decls = vec![
            action("pa", "add", &[Place::TabMenu, Place::TopBar]),
            action("pb", "bare", &[Place::TopBar]),
            action("pc", "x", &[Place::TopBar]),
            action("pd", "y", &[Place::TopBar]),
        ];
        let mut ui = PluginUi::default();
        ui.set_action(
            "pa",
            "add",
            None,
            &ActionState {
                checked: Some(true),
                title: Some("Added".into()),
                ..ActionState::default()
            },
        );
        ui.set_action(
            "pb",
            "bare",
            None,
            &ActionState {
                enabled: Some(false),
                ..ActionState::default()
            },
        );
        let top = buttons(&decls, &ui, Place::TopBar, None, TOP_BAR_MAX, &no_glyph);
        assert_eq!(top.len(), TOP_BAR_MAX, "three at most in the top bar");
        assert_eq!(top[0].tip, "Plugin pa: Added");
        assert!(top[0].checked && top[0].enabled);
        assert_eq!(top[0].cmd, "path:plus");
        assert_eq!((top[1].cmd.as_str(), top[1].letter.as_str()), ("", "D"));
        assert!(!top[1].enabled);
        assert!(
            buttons(
                &decls,
                &ui,
                Place::TabHover,
                Some("t"),
                HOVER_MAX,
                &no_glyph
            )
            .is_empty()
        );
    }

    #[test]
    fn the_tab_menu_has_one_group_per_plugin_under_its_name() {
        let decls = vec![
            action("pa", "one", &[Place::TabMenu]),
            action("pa", "two", &[Place::TabMenu, Place::TabHover]),
            action("pb", "three", &[Place::TabMenu]),
            action("pc", "top", &[Place::TopBar]),
        ];
        let ui = PluginUi::default();
        let g = menu_groups(&decls, &ui, "t", &no_glyph);
        let names: Vec<(&str, usize)> = g.iter().map(|(n, l)| (n.as_str(), l.len())).collect();
        assert_eq!(names, [("Plugin pa", 2), ("Plugin pb", 1)]);
        assert_eq!(g[0].1[1].title, "do two");
    }
}
