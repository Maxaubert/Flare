//! The plugins' session bar in the window (plugins phase 6, design 5.5 and 13): the part of `AppCtl` that keeps which
//! plugin's bar each tab shows (`plugin_bar`), gives the front tab's height to the layout and its page to the browser,
//! and turns the person's clicks on the bar's chips into the plugin's `action` events.
//!
//! - `bar.show` and `bar.hide` are checked on the worker (`CtlOp::BarShow`, `CtlOp::BarHide`) and applied here for the
//!   plugin generation they were checked for. One bar a tab: another plugin's gets `busy`.
//! - The terminal gives up the bar's height: the grid is laid out shorter and the pty is resized once, by the same
//!   path a window resize takes (`grid-resized`). No animation, so no frame shows it half way.
//! - The bar's page is the plugin's (`bar.entry`), sealed as a bar (no menu, no browser keys, no download, never the
//!   keyboard), networked only with an approved `network`. It is made the first time its tab is in front, hidden while
//!   the tab is behind or a Flare overlay is open, and it hears the plugin's events like its other pages.
//! - The plugin's `session-bar` actions are drawn as Flare's chips at the bar's right end, at most
//!   [`plugin_bar::CHIPS_MAX`]; a click is the event `action` `{key, tab}` with click scope on the tab.
//! - Every bar of a plugin goes when it stops (turned off, its background page ended); a tab's goes with it; a bar
//!   page that ends takes its bar off the tab (the plugin may show it again).

use flare_ctl::{CtlError, code};
use flare_mods::manifest::Place;
use flare_surfaces::modview;
use flare_surfaces::slot::Slot;
use serde_json::{Value, json};

use super::AppCtl;
use super::plugins_glue::sync_model;
use crate::plugin_bar::{self, Refused};
use crate::plugin_ui::{self, Button};
use crate::web_ctl::{BarChange, BarWant, WebOut};
use crate::{AppWindow, PluginButton};

/// The bars as the tests read them: (tab, plugin, height), and the browser's views (tab, plugin, on screen).
#[cfg(test)]
pub(crate) type BarState = (Vec<(String, String, u32)>, Vec<(String, String, bool)>);

impl AppCtl {
    /// `bar.show`: the plugin's bar on the tab, if it is still on in the generation that was checked.
    pub(super) fn ctl_bar_show(
        &mut self,
        plugin: String,
        tab: String,
        generation: u64,
    ) -> Result<Value, CtlError> {
        self.plugin_still_on(&plugin, generation)?;
        if !self.is_shell_tab(&tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        let height = self
            .mods
            .as_ref()
            .and_then(|m| m.hub().bar_of(&plugin))
            .map(|(_, _, h)| h)
            .ok_or_else(|| CtlError::new(code::NOT_FOUND, "the plugin has no bar it may show"))?;
        match self.bars.show(&tab, &plugin, height) {
            Ok(changed) => {
                if changed {
                    log::info!("plugin {plugin}: its bar shows on a tab");
                    self.refresh();
                }
                Ok(json!({"tab": tab, "height": self.bars.height(Some(&tab))}))
            }
            Err(Refused::Busy(owner)) => {
                let name = self
                    .mods
                    .as_ref()
                    .map_or_else(|| owner.clone(), |m| m.hub().name_of(&owner));
                Err(CtlError::new(
                    code::BUSY,
                    format!("that tab shows the bar of {name}"),
                ))
            }
        }
    }

    /// `bar.hide`: the plugin's bar leaves the tab.
    pub(super) fn ctl_bar_hide(&mut self, plugin: String, tab: String) -> Result<Value, CtlError> {
        if !self.is_shell_tab(&tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        let hidden = self.bars.hide(&tab, &plugin);
        if hidden {
            self.refresh();
        }
        Ok(json!({"hidden": hidden}))
    }

    /// Every bar of the plugin goes (it stopped).
    pub(super) fn clear_plugin_bars(&mut self, id: &str) {
        if !self.bars.clear_plugin(id).is_empty() {
            self.refresh();
        }
    }

    /// A tab closed: its bar goes with it (its view goes at the next sync).
    pub(super) fn bar_tab_gone(&mut self, tab: &str) {
        self.bars.remove_tab(tab);
    }

    /// The front shell tab whose grid shows now, if any.
    fn bar_front(&self, app: &AppWindow) -> Option<String> {
        if app.get_front_kind() != 0 {
            return None;
        }
        self.model.active_shell().map(|t| t.id.clone())
    }

    /// The front tab's bar height and chips, to the layout (before the browser is synced, so the bar's rectangle is
    /// the new one). Only a change reaches Slint.
    pub(super) fn push_bar(&mut self, app: &AppWindow) {
        let front = self.bar_front(app);
        let height = self.bars.height(front.as_deref());
        let px = height as f32;
        if (app.get_bar_h() - px).abs() > f32::EPSILON {
            app.set_bar_h(px);
        }
        let chips = match (&front, front.as_deref().and_then(|t| self.bars.of(t))) {
            (Some(tab), Some(hold)) => {
                let p = &self.plugins;
                let mine: Vec<plugin_ui::ActionDecl> = p
                    .actions
                    .iter()
                    .filter(|a| a.plugin == hold.plugin)
                    .cloned()
                    .collect();
                let glyph = |plugin: &str, icon: &str| p.glyph(plugin, icon);
                plugin_ui::buttons(
                    &mine,
                    &p.ui,
                    Place::SessionBar,
                    Some(tab),
                    plugin_bar::CHIPS_MAX,
                    &glyph,
                )
            }
            _ => Vec::new(),
        };
        if chips != self.bar_chips {
            let rows: Vec<PluginButton> = chips
                .iter()
                .map(|b| PluginButton {
                    cmd: b.cmd.as_str().into(),
                    letter: b.letter.as_str().into(),
                    tip: b.tip.as_str().into(),
                    checked: b.checked,
                    enabled: b.enabled,
                })
                .collect();
            sync_model(&self.bar_chips_model, &rows);
            self.bar_chips = chips;
        }
    }

    /// The bar views follow the bars the tabs show: the front one on screen, the others hidden or closed. A view that
    /// opened gets its page bound (a fresh generation), one that closed lets its page go.
    pub(super) fn sync_bar_views(&mut self, app: &AppWindow) {
        if self.bars.is_empty() && self.web.bar_views().is_empty() {
            return;
        }
        let wanted: Vec<BarWant> = match self.mods.as_ref() {
            Some(m) => {
                let hub = m.hub();
                self.bars
                    .all()
                    .into_iter()
                    .filter_map(|(tab, hold)| {
                        let (folder, entry, _) = hub.bar_of(&hold.plugin)?;
                        let net = hub.network_of(&hold.plugin);
                        Some(BarWant {
                            tab,
                            spec: modview::plugin_spec(&hold.plugin, &folder, &entry, net),
                            plugin: hold.plugin,
                        })
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let front = self.bar_front(app);
        let changes = self.web.sync_bars(app, &wanted, front.as_deref());
        for c in changes {
            match c {
                BarChange::Opened { tab, plugin } => {
                    if let Some(m) = self.mods.as_mut() {
                        m.bind(Slot::bar(&tab), &plugin);
                    }
                }
                BarChange::Closed { tab, plugin } => {
                    if let Some(m) = self.mods.as_mut()
                        && m.mod_of(Slot::bar(&tab)) == Some(plugin.as_str())
                    {
                        m.unbind(Slot::bar(&tab));
                    }
                }
            }
        }
    }

    /// What the browser said of a bar page: its messages run with the plugin's live grant and the tab it sits in, its
    /// load makes it ready (and hands it what waited), and its end takes the bar off the tab.
    pub(super) fn on_bar_web(&mut self, out: WebOut) {
        match out {
            WebOut::BarMessage { tab, plugin, json } => {
                let slot = Slot::bar(&tab);
                // The message counts only for the plugin whose bar the tab shows now.
                if self.bars.of(&tab).map(|h| h.plugin.as_str()) != Some(plugin.as_str()) {
                    return;
                }
                let reply = self
                    .mods
                    .as_mut()
                    .filter(|m| m.mod_of(&slot) == Some(plugin.as_str()))
                    .and_then(|m| m.on_page_message(&slot, &json));
                if let Some(r) = reply {
                    self.web.post_to_bar(&tab, &r);
                }
            }
            WebOut::BarPage { tab, plugin, ready } => {
                let slot = Slot::bar(&tab);
                let queued = match self.mods.as_mut() {
                    Some(m) if m.mod_of(&slot) == Some(plugin.as_str()) => {
                        m.set_ready(&slot, ready)
                    }
                    _ => Vec::new(),
                };
                for q in queued {
                    self.web.post_to_bar(&tab, &q);
                }
            }
            WebOut::BarLost { tab, plugin } => {
                if let Some(m) = self.mods.as_mut()
                    && m.mod_of(Slot::bar(&tab)) == Some(plugin.as_str())
                {
                    m.unbind(Slot::bar(&tab));
                }
                log::warn!("plugin {plugin}: its bar page ended");
                if self.bars.hide(&tab, &plugin) {
                    self.refresh();
                }
            }
            _ => {}
        }
    }

    /// A chip of the front tab's bar was clicked: the plugin's `session-bar` action on that tab.
    pub(super) fn bar_chip_clicked(&mut self, i: i32) {
        let Some(b): Option<Button> = usize::try_from(i)
            .ok()
            .and_then(|i| self.bar_chips.get(i))
            .cloned()
        else {
            return;
        };
        let Some(tab) = self
            .model
            .active_shell()
            .map(|t| t.id.clone())
            .filter(|t| self.bars.of(t).is_some_and(|h| h.plugin == b.plugin))
        else {
            return;
        };
        self.plugin_action(&b.plugin, &b.key, Some(&tab));
    }

    /// The bars and their views, for the tests: (tab, plugin, height) and the browser's (tab, plugin, on screen).
    #[cfg(test)]
    pub(crate) fn bar_state(&self) -> BarState {
        (
            self.bars
                .all()
                .into_iter()
                .map(|(t, h)| (t, h.plugin, h.height))
                .collect(),
            self.web.bar_views(),
        )
    }
}
