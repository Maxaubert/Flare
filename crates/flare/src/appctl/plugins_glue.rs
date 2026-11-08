//! The plugins' sidebar sections and actions in the window (plugins phase 4, design 5.3, 5.4 and 13): the part of
//! `AppCtl` that turns what the plugins declare and set (`plugin_ui`) into rows, menus and buttons, and the person's
//! clicks on them into events.
//!
//! - What the plugins that are on declare is read from the hub when the plugins change ([`AppCtl::sync_plugin_surfaces`]),
//!   never at each refresh; what they set comes through the channel (`CtlOp::SidebarSet`, `CtlOp::ActionsSet`),
//!   checked on the worker, and applied here only for the plugin generation it was checked for.
//! - Sections: drawn in the sidebar in the vertical layout and in the Plugins dropdown in the horizontal one, in the
//!   person's order (the header menu moves one up or down; the order and the folded ones are preferences).
//! - Actions: a flyout under the plugin's name in the tab's menu, a hover button on the tab, a button in the title
//!   bar. A click is the event `action` `{key, tab}` for the plugin's background page (else its other pages that
//!   listen); a click or a tick on an item is `sidebar.item` `{key, item, checked?}`.
//! - Click scope (design 13): a click on tab X (a tab's menu or hover button, an item that leads to X, a top-bar
//!   button while X is in front) lets that plugin act on X until X closes or the plugin stops.
//! - Everything a plugin set goes when it stops, and what a closed tab carried goes with it.
//! - Agent icon motions (owner decision 2026-10-26): a plugin with `agent-looks` picks how an agent's icon moves while
//!   it works (`CtlOp::MotionSet`, `CtlOp::MotionClear`); the newest pick wins, and a plugin that stops takes its own.

use std::collections::{HashMap, HashSet};

use flare_ctl::{CtlError, Grant, Permission, code};
use flare_mods::bridge;
use flare_mods::manifest::Place;
use flare_surfaces::slot::{Slot, ViewKind};
use serde_json::{Value, json};
use slint::Model;

use super::AppCtl;
use crate::menus_ui::{MenuKind, MenuRowData, badge_glyph, glyph_path};
use crate::plugin_ui::{
    self, ActionDecl, ActionState, Button, HOVER_MAX, Item, SectionDecl, TOP_BAR_MAX,
};
use crate::{AppWindow, PluginButton, PluginRow};
use flare_agent::icon_motion::IconMotion;
use flare_agent::kinds::AgentKind;

/// What a tab-menu row of a plugin's action carries as its path: the plugin and the key, apart from Flare's own.
const ACTION_ROW: &str = "plugin-action\u{1f}";
/// The tick a ticked menu action wears.
const TICK: &str = "M5 12.5l4.5 4.5L19 7.5";
/// The section menu's rows.
pub const SECTION_UP: &str = "Move up";
pub const SECTION_DOWN: &str = "Move down";
pub const SECTION_FOLD: &str = "Fold";
pub const SECTION_UNFOLD: &str = "Unfold";

/// What the window knows of the plugins' sections and actions: what they declare (from the hub), what they set,
/// the person's order and folds, and what was drawn last (a click is matched against it).
#[derive(Default)]
pub struct PluginSurfaces {
    pub ui: plugin_ui::PluginUi,
    pub sections: Vec<SectionDecl>,
    pub actions: Vec<ActionDecl>,
    /// Each plugin's own icons, by name.
    pub icons: HashMap<String, HashMap<String, String>>,
    /// The person's order of the sections and the folded ones, read from the preferences once.
    pub order: Option<Vec<String>>,
    pub folded: HashSet<String>,
    /// The Plugins dropdown is open (horizontal tabs).
    pub open: bool,
    /// The section ids drawn last, in order, and the top bar's buttons.
    pub drawn: Vec<String>,
    pub top: Vec<Button>,
}

impl PluginSurfaces {
    /// The path of a plugin's icon: a built-in badge, or one of its own `contributes.icons`; "" for neither.
    pub fn glyph(&self, plugin: &str, icon: &str) -> String {
        match badge_glyph(icon) {
            Some(g) => glyph_path(g).to_string(),
            None => self
                .icons
                .get(plugin)
                .and_then(|m| m.get(icon))
                .cloned()
                .unwrap_or_default(),
        }
    }
}

impl AppCtl {
    /// Reads again what the plugins that are on declare (they were turned on or off, a scan ran, a background page
    /// stopped). Sections and actions of a plugin that is no longer on go from the screen.
    pub(super) fn sync_plugin_surfaces(&mut self) {
        let surfaces = self
            .mods
            .as_ref()
            .map(|m| m.hub().surfaces())
            .unwrap_or_default();
        let p = &mut self.plugins;
        if p.order.is_none() {
            p.order = Some(plugin_ui::parse_ids(
                &self.prefs.get(plugin_ui::ORDER_PREF).unwrap_or_default(),
            ));
            p.folded =
                plugin_ui::parse_ids(&self.prefs.get(plugin_ui::FOLDED_PREF).unwrap_or_default())
                    .into_iter()
                    .collect();
        }
        p.sections = surfaces
            .iter()
            .flat_map(|s| {
                s.sidebar.iter().map(|d| SectionDecl {
                    plugin: s.id.clone(),
                    plugin_name: s.name.clone(),
                    key: d.key.clone(),
                    title: d.title.clone(),
                })
            })
            .collect();
        p.actions = surfaces
            .iter()
            .flat_map(|s| {
                s.actions.iter().map(|a| ActionDecl {
                    plugin: s.id.clone(),
                    plugin_name: s.name.clone(),
                    key: a.key.clone(),
                    title: a.title.clone(),
                    icon: a.icon.clone(),
                    places: a.places.clone(),
                })
            })
            .collect();
        p.icons = surfaces
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.icons
                        .iter()
                        .map(|i| (i.name.clone(), i.path.clone()))
                        .collect(),
                )
            })
            .collect();
    }

    /// Everything the plugin set goes (it stopped, or its background page ended), its bars too (plugins phase 6), and
    /// its picks of the agents' motions.
    pub(super) fn clear_plugin_surfaces(&mut self, id: &str) {
        if self.plugins.ui.clear_plugin(id) {
            self.refresh();
        }
        self.clear_plugin_bars(id);
        if self.motions.clear_plugin(id) {
            self.refresh();
        }
    }

    /// `agents.motion.set`: the plugin's pick for the agent, if it is still on in the generation that was checked.
    /// Answers the motion the agent wears now (this pick: the newest wins).
    pub(super) fn ctl_motion_set(
        &mut self,
        plugin: String,
        agent: AgentKind,
        motion: IconMotion,
        generation: u64,
    ) -> Result<Value, CtlError> {
        self.plugin_still_on(&plugin, generation)?;
        if self.motions.set(agent, &plugin, motion) {
            self.refresh();
        }
        Ok(json!({"agent": agent.as_str(), "motion": self.motions.of(agent).name()}))
    }

    /// `agents.motion.clear`: the plugin's pick for the agent goes. Answers whether it had one and what the agent wears
    /// now (another plugin's older pick, or `none`).
    pub(super) fn ctl_motion_clear(
        &mut self,
        plugin: String,
        agent: AgentKind,
    ) -> Result<Value, CtlError> {
        let cleared = self.motions.clear(agent, &plugin);
        if cleared {
            self.refresh();
        }
        Ok(json!({
            "agent": agent.as_str(),
            "cleared": cleared,
            "motion": self.motions.of(agent).name(),
        }))
    }

    /// A tab closed: the action state set for it goes, and no plugin keeps click scope on it.
    pub(super) fn plugin_tab_gone(&mut self, tab: &str) {
        self.plugins.ui.remove_tab(tab);
        self.bar_tab_gone(tab);
        if let Some(m) = &self.mods {
            m.hub().forget_tab(tab);
        }
    }

    /// The hover buttons each tab draws (before the rows go to Slint).
    pub(super) fn plugin_hover_rows(&self, rows: &mut [crate::tabs_ui::RowData]) {
        let p = &self.plugins;
        if !p
            .actions
            .iter()
            .any(|a| a.places.contains(&Place::TabHover))
        {
            for r in rows.iter_mut() {
                r.hover.clear();
            }
            return;
        }
        let glyph = |plugin: &str, icon: &str| p.glyph(plugin, icon);
        for r in rows.iter_mut() {
            r.hover = if r.settings {
                Vec::new()
            } else {
                plugin_ui::buttons(
                    &p.actions,
                    &p.ui,
                    Place::TabHover,
                    Some(&r.id),
                    HOVER_MAX,
                    &glyph,
                )
            };
        }
    }

    /// The sections' rows, the top bar's buttons and the dropdown, to the window (in place: the models are kept).
    pub(super) fn push_plugins(&mut self, app: &AppWindow) {
        let p = &mut self.plugins;
        let order = p.order.clone().unwrap_or_default();
        let drawn = plugin_ui::ordered(&p.sections, &order);
        let rows = {
            let glyph = |plugin: &str, icon: &str| p.glyph(plugin, icon);
            plugin_ui::rows(&drawn, &p.ui, &p.folded, &glyph)
        };
        p.drawn = drawn.iter().map(|d| d.sid()).collect();
        let top = {
            let glyph = |plugin: &str, icon: &str| p.glyph(plugin, icon);
            plugin_ui::buttons(&p.actions, &p.ui, Place::TopBar, None, TOP_BAR_MAX, &glyph)
        };
        p.top = top;
        // Nothing to list, or the sections are in the sidebar now: the dropdown is not open (it would hide a web
        // panel for nothing).
        if p.drawn.is_empty() || self.vertical() {
            self.plugins.open = false;
        }
        let rows: Vec<PluginRow> = rows
            .iter()
            .map(|r| PluginRow {
                kind: r.kind,
                sid: r.sid.as_str().into(),
                id: r.id.as_str().into(),
                text: r.text.as_str().into(),
                sub: r.sub.as_str().into(),
                icon: r.icon.as_str().into(),
                check: r.check,
                jump: r.jump,
                folded: r.folded,
                can_up: r.can_up,
                can_down: r.can_down,
            })
            .collect();
        sync_model(&self.plugin_rows_model, &rows);
        let top: Vec<PluginButton> = self
            .plugins
            .top
            .iter()
            .map(|b| PluginButton {
                cmd: b.cmd.as_str().into(),
                letter: b.letter.as_str().into(),
                tip: b.tip.as_str().into(),
                checked: b.checked,
                enabled: b.enabled,
            })
            .collect();
        sync_model(&self.plugin_top_model, &top);
        app.set_plugins_open(self.plugins.open);
    }

    /// The plugins' rows in a shell tab's menu (2026-11-08, the same rule for every plugin): a plugin with ONE action
    /// there shows it flat, its title beside the plugin's logo; one with more has a flyout under its name, the logo on
    /// that entry and its actions in it. The logo is the brand's (a plugin with none shows its action's own icon, or
    /// Flare's chip on the flyout entry); a ticked action wears the tick instead.
    pub(super) fn plugin_menu_rows(&self, tab: &str, rows: &mut Vec<(MenuRowData, String)>) {
        let p = &self.plugins;
        let glyph = |plugin: &str, icon: &str| p.glyph(plugin, icon);
        let groups = plugin_ui::menu_groups(&p.actions, &p.ui, tab, &glyph);
        let logo_of = |plugin: &str| -> Option<(String, String)> {
            let brand = self.mods.as_ref()?.hub().found(plugin)?.manifest.brand?;
            Some((brand.icon?, brand.color))
        };
        let chip = flare_settings::icons::icon_path("chip").unwrap_or("");
        for (i, (name, buttons)) in groups.into_iter().enumerate() {
            let logo = buttons.first().and_then(|b| logo_of(&b.plugin));
            if let [b] = buttons.as_slice() {
                let mut r = MenuRowData::labelled(&b.title);
                r.group = i == 0;
                r.disabled = !b.enabled;
                if b.checked {
                    r.icon_cmd = TICK.to_string();
                } else if logo.is_some() {
                    r.logo = logo;
                } else {
                    r.icon_cmd = if b.cmd.is_empty() {
                        chip.to_string()
                    } else {
                        b.cmd.clone()
                    };
                }
                rows.push((r, format!("{ACTION_ROW}{}\u{1f}{}", b.plugin, b.key)));
                continue;
            }
            let parent = rows.len() as i32;
            let mut head = MenuRowData::labelled(&name);
            head.flyout = true;
            head.group = i == 0;
            if logo.is_some() {
                head.logo = logo;
            } else {
                head.icon_cmd = chip.to_string();
            }
            rows.push((head, String::new()));
            for b in buttons {
                let mut r = MenuRowData::labelled(&b.title);
                r.parent = parent;
                r.disabled = !b.enabled;
                r.icon_cmd = if b.checked { TICK.to_string() } else { b.cmd };
                rows.push((r, format!("{ACTION_ROW}{}\u{1f}{}", b.plugin, b.key)));
            }
        }
    }

    /// A pick in a tab's menu that is a plugin's action: handled (true), or one of Flare's own rows (false).
    pub(super) fn plugin_menu_picked(&mut self, tab: &str, path: &str) -> bool {
        let Some(rest) = path.strip_prefix(ACTION_ROW) else {
            return false;
        };
        if let Some((plugin, key)) = rest.split_once('\u{1f}') {
            let (plugin, key) = (plugin.to_string(), key.to_string());
            self.plugin_action(&plugin, &key, Some(tab));
        }
        true
    }

    /// A tab's hover button was clicked.
    pub(super) fn tab_hover_action(&mut self, tab: &str, i: i32) {
        let b = self
            .view
            .rows
            .iter()
            .find(|r| r.id == tab)
            .and_then(|r| usize::try_from(i).ok().and_then(|i| r.hover.get(i)))
            .cloned();
        if let Some(b) = b {
            self.plugin_action(&b.plugin, &b.key, Some(tab));
        }
    }

    /// A top-bar button was clicked: it acts on the shell tab in front, if there is one.
    pub(super) fn plugin_top_clicked(&mut self, i: i32) {
        let Some(b) = usize::try_from(i)
            .ok()
            .and_then(|i| self.plugins.top.get(i))
            .cloned()
        else {
            return;
        };
        let front = self.model.active_shell().map(|t| t.id.clone());
        self.plugin_action(&b.plugin, &b.key, front.as_deref());
    }

    /// The person clicked the plugin's action `key` on `tab` (None: no shell tab is in front). An action turned off
    /// there does nothing; else the plugin gets click scope on the tab and hears `action` `{key, tab}`.
    pub(super) fn plugin_action(&mut self, plugin: &str, key: &str, tab: Option<&str>) {
        let declared = self
            .plugins
            .actions
            .iter()
            .any(|a| a.plugin == plugin && a.key == key);
        if !declared {
            return;
        }
        let state = self.plugins.ui.action(plugin, key, tab);
        if state.enabled == Some(false) {
            return;
        }
        let tab = tab.filter(|t| self.is_shell_tab(t));
        if let (Some(t), Some(m)) = (tab, &self.mods) {
            m.hub().grant_click_scope(plugin, t);
        }
        self.plugin_event(plugin, "action", json!({"key": key, "tab": tab}));
    }

    /// An item of a section was clicked (`on_box`: on its checkbox, which ticks it). An item that leads to a tab
    /// brings that tab to the front (a click beside the box), and gives the plugin click scope on it.
    pub(super) fn plugin_item(&mut self, sid: &str, id: &str, on_box: bool) {
        let Some((plugin, key)) = plugin_ui::split_section(sid) else {
            return;
        };
        let (plugin, key) = (plugin.to_string(), key.to_string());
        if !self.plugins.drawn.iter().any(|d| d == sid) {
            return;
        }
        let Some(item) = self.plugins.ui.item(sid, id).cloned() else {
            return;
        };
        let checked = if on_box {
            self.plugins.ui.toggle(sid, id)
        } else {
            None
        };
        let tab = item.tab.clone().filter(|t| self.is_shell_tab(t));
        if let (Some(t), Some(m)) = (&tab, &self.mods) {
            m.hub().grant_click_scope(&plugin, t);
        }
        let mut data = json!({"key": key, "item": id});
        if let Some(c) = checked {
            data["checked"] = json!(c);
        }
        self.plugin_event(&plugin, "sidebar.item", data);
        match tab {
            Some(t) if !on_box => {
                self.plugins.open = false;
                self.model.pick(&t);
                self.refresh();
                self.focus_terminal();
            }
            _ => self.refresh(),
        }
    }

    /// A section's header was clicked: it folds, or unfolds.
    pub(super) fn plugin_fold(&mut self, sid: &str) {
        if !self.plugins.drawn.iter().any(|d| d == sid) {
            return;
        }
        if !self.plugins.folded.remove(sid) {
            self.plugins.folded.insert(sid.to_string());
        }
        let mut ids: Vec<String> = self.plugins.folded.iter().cloned().collect();
        ids.sort();
        self.prefs
            .set(plugin_ui::FOLDED_PREF, &plugin_ui::join_ids(&ids));
        self.refresh();
    }

    /// A removed plugin's sections leave the person's order and folds, so one installed again with its id starts
    /// afresh (design 13).
    pub(super) fn forget_plugin_sections(&mut self, plugin: &str) {
        let theirs = |sid: &String| plugin_ui::split_section(sid).is_some_and(|(p, _)| p == plugin);
        // Read from the prefs until the sidebar first loads them.
        let loaded = self.plugins.order.is_some();
        let (order, folded) = if loaded {
            let order = self.plugins.order.clone().unwrap_or_default();
            (order, self.plugins.folded.iter().cloned().collect())
        } else {
            let pref = |key| plugin_ui::parse_ids(&self.prefs.get(key).unwrap_or_default());
            (pref(plugin_ui::ORDER_PREF), pref(plugin_ui::FOLDED_PREF))
        };
        if order.iter().any(theirs) {
            let kept: Vec<String> = order.into_iter().filter(|s| !theirs(s)).collect();
            self.prefs
                .set(plugin_ui::ORDER_PREF, &plugin_ui::join_ids(&kept));
            if loaded {
                self.plugins.order = Some(kept);
            }
        }
        if folded.iter().any(theirs) {
            let mut kept: Vec<String> = folded.into_iter().filter(|s| !theirs(s)).collect();
            kept.sort();
            self.prefs
                .set(plugin_ui::FOLDED_PREF, &plugin_ui::join_ids(&kept));
            if loaded {
                self.plugins.folded = kept.into_iter().collect();
            }
        }
    }

    /// A section's header menu: move it up, move it down, fold or unfold it.
    pub(super) fn plugin_header_menu(&mut self, sid: &str, x: f32, y: f32) {
        let Some(i) = self.plugins.drawn.iter().position(|d| d == sid) else {
            return;
        };
        let n = self.plugins.drawn.len();
        let mut up = MenuRowData::labelled(SECTION_UP);
        up.disabled = i == 0;
        let mut down = MenuRowData::labelled(SECTION_DOWN);
        down.disabled = i + 1 >= n;
        let fold = MenuRowData::labelled(if self.plugins.folded.contains(sid) {
            SECTION_UNFOLD
        } else {
            SECTION_FOLD
        });
        let rows = vec![up, down, fold]
            .into_iter()
            .map(|r| (r, String::new()))
            .collect();
        self.show_menu(
            MenuKind::Section {
                sid: sid.to_string(),
            },
            rows,
            x,
            y,
        );
    }

    /// A pick in a section's menu.
    pub(super) fn section_menu_picked(&mut self, sid: &str, label: &str) {
        match label {
            SECTION_UP | SECTION_DOWN => {
                let order = self.plugins.order.clone().unwrap_or_default();
                if let Some(next) =
                    plugin_ui::moved(&self.plugins.drawn, &order, sid, label == SECTION_UP)
                {
                    self.prefs
                        .set(plugin_ui::ORDER_PREF, &plugin_ui::join_ids(&next));
                    self.plugins.order = Some(next);
                }
            }
            SECTION_FOLD | SECTION_UNFOLD => self.plugin_fold(sid),
            _ => {}
        }
    }

    /// The top bar's Plugins button: the dropdown opens, or closes.
    pub(super) fn plugins_clicked(&mut self) {
        self.close_menu_quietly();
        self.close_overflow_quietly();
        self.clear_tip();
        self.plugins.open = !self.plugins.open && !self.plugins.drawn.is_empty();
        self.refresh();
    }

    pub(super) fn plugins_dismissed(&mut self) {
        if self.plugins.open {
            self.plugins.open = false;
            self.refresh();
            self.focus_terminal();
        }
    }

    /// Hands `data` as the event `name` to the plugin's pages that listen: an `action` to its background page when
    /// that one listens, else to every page of it that does. Nothing for a plugin that no longer holds the
    /// permission the event belongs to.
    pub(super) fn plugin_event(&mut self, plugin: &str, name: &str, data: Value) {
        let Some(m) = self.mods.as_mut() else { return };
        let needs = match name {
            "action" => Permission::Actions,
            _ => Permission::Sidebar,
        };
        let holds = matches!(
            m.hub().grant_of_mod(plugin),
            Some(Grant::Mod { permissions, .. }) if permissions.contains(&needs)
        );
        if !holds {
            return;
        }
        let mut slots: Vec<Slot> = m
            .listeners(name)
            .into_iter()
            .filter(|(_, id)| id == plugin)
            .map(|(s, _)| s)
            .collect();
        if name == "action" && slots.iter().any(|s| s.kind == ViewKind::Background) {
            slots.retain(|s| s.kind == ViewKind::Background);
        }
        let msg = bridge::event_message(name, &data);
        let mut out = Vec::new();
        for slot in slots {
            if let Some(now) = m.queue_or_send(&slot, msg.clone()) {
                out.push((slot, now));
            }
        }
        for (slot, msg) in out {
            self.post_to_slot(&slot, &msg);
        }
    }

    /// `sidebar.set` on the UI thread: the worker checked it; the plugin must still be on in the generation it was
    /// checked for, and the section still drawn for it.
    pub(super) fn ctl_sidebar_set(
        &mut self,
        plugin: String,
        key: String,
        items: Vec<Item>,
        generation: u64,
    ) -> Result<Value, CtlError> {
        self.plugin_still_on(&plugin, generation)?;
        if !self
            .plugins
            .sections
            .iter()
            .any(|s| s.plugin == plugin && s.key == key)
        {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                "the section is not shown now",
            ));
        }
        if self.plugins.ui.set_items(&plugin, &key, items) {
            self.refresh();
        }
        Ok(json!({}))
    }

    /// `actions.set` on the UI thread: as `sidebar.set`, and a tab it names must still be open.
    pub(super) fn ctl_actions_set(
        &mut self,
        plugin: String,
        key: String,
        tab: Option<String>,
        state: ActionState,
        generation: u64,
    ) -> Result<Value, CtlError> {
        self.plugin_still_on(&plugin, generation)?;
        if let Some(t) = &tab
            && !self.is_shell_tab(t)
        {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        if self
            .plugins
            .ui
            .set_action(&plugin, &key, tab.as_deref(), &state)
        {
            self.refresh();
        }
        Ok(json!({}))
    }

    /// A change checked while the plugin was on is not applied once it is off, nor after it was switched off and
    /// on again (its generation moved on).
    pub(super) fn plugin_still_on(&self, plugin: &str, generation: u64) -> Result<(), CtlError> {
        let on = self.mods.as_ref().is_some_and(|m| {
            m.hub().grant_of_mod(plugin).is_some() && m.hub().generation(plugin) == generation
        });
        if on {
            Ok(())
        } else {
            Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin is turned off",
            ))
        }
    }

    /// What the sections and buttons show now, for the tests and the probe.
    #[cfg(test)]
    pub(crate) fn plugin_rows(&self) -> Vec<PluginRow> {
        (0..self.plugin_rows_model.row_count())
            .filter_map(|i| self.plugin_rows_model.row_data(i))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn plugin_top(&self) -> Vec<Button> {
        self.plugins.top.clone()
    }
}

/// Puts `rows` into the model in place: rows that differ are set, extra ones pushed or removed from the end (a new
/// model would drop the rows' hover and touch state).
pub(super) fn sync_model<T: Clone + PartialEq + 'static>(target: &slint::VecModel<T>, rows: &[T]) {
    let shared = target.row_count().min(rows.len());
    for (i, row) in rows.iter().enumerate().take(shared) {
        if target.row_data(i).as_ref() != Some(row) {
            target.set_row_data(i, row.clone());
        }
    }
    for row in rows.iter().skip(shared) {
        target.push(row.clone());
    }
    while target.row_count() > rows.len() {
        target.remove(target.row_count() - 1);
    }
}
