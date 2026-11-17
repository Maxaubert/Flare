//! The plugins' notices in the window (`notice.show`, rule 1275, 2026-11-17, Max's approved mockup A): a short line under
//! the title bar with the plugin's logo and a close button, in the staged toast's place and look.
//!
//! The call's thread checked the grant (`actions`), the params, the text and the plugin's three in ten seconds. Here a
//! plugin turned off since (or off and on again) shows nothing; else its notice goes up in place of the one it had. A
//! repeating tick counts each notice down while the pointer is off it, and only while one shows. The plugin's text is
//! never logged.

use flare_ctl::{CtlError, code};
use serde_json::{Value, json};
use slint::{ModelRc, VecModel};

use super::AppCtl;
use crate::PluginNoticeRow;
use crate::plugin_notice::TICK_MS;

const NOTICE_TIMER: &str = "plugins.notice";

impl AppCtl {
    /// `notice.show` on the window's side.
    pub(super) fn ctl_notice_show(
        &mut self,
        plugin: &str,
        text: &str,
        error: bool,
        generation: u64,
    ) -> Result<Value, CtlError> {
        if !self.plugin_live(plugin, generation) {
            return Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin was turned off",
            ));
        }
        log::info!(
            "notice.show: plugin {plugin}: {}",
            if error { "error" } else { "info" }
        );
        self.plugin_notices.show(plugin, text, error);
        self.sync_plugin_notices();
        Ok(json!({"shown": true}))
    }

    /// The person closed a notice with its close button.
    pub(super) fn plugin_notice_closed(&mut self, id: &str) {
        if self.plugin_notices.close(id) {
            self.clear_tip();
            self.sync_plugin_notices();
        }
    }

    /// The pointer came onto a notice or left it: its time stands still while it is there.
    pub(super) fn plugin_notice_hover(&mut self, id: &str, on: bool) {
        self.plugin_notices.hover(id, on);
    }

    /// The plugin was turned off: its notice goes.
    pub(super) fn plugin_notices_off(&mut self, id: &str) {
        if self.plugin_notices.close(id) {
            self.sync_plugin_notices();
        }
    }

    fn plugin_notice_tick(&mut self) {
        if self.plugin_notices.tick(TICK_MS) {
            self.sync_plugin_notices();
        }
    }

    /// The window shows the notices as they stand; the tick runs while any shows.
    fn sync_plugin_notices(&mut self) {
        let rows: Vec<PluginNoticeRow> = self
            .plugin_notices
            .list()
            .iter()
            .map(|n| {
                let found = self.mods.as_ref().and_then(|m| m.hub().found(&n.plugin));
                let name = found
                    .as_ref()
                    .map_or_else(|| n.plugin.clone(), |f| f.manifest.name.clone());
                let brand = found.and_then(|f| f.manifest.brand);
                PluginNoticeRow {
                    id: n.plugin.as_str().into(),
                    name: name.as_str().into(),
                    text: n.text.as_str().into(),
                    error: n.error,
                    look: crate::plugin_look::look_of(brand.as_ref()),
                }
            })
            .collect();
        if let Some(app) = self.app() {
            app.set_plugin_notices(ModelRc::new(VecModel::from(rows)));
        }
        if self.plugin_notices.is_empty() {
            crate::timer_registry::clear(NOTICE_TIMER);
        } else if !crate::timer_registry::running(NOTICE_TIMER) {
            let weak = self.me.clone();
            crate::timer_registry::set(NOTICE_TIMER, TICK_MS, true, move || {
                if let Some(rc) = weak.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.plugin_notice_tick();
                }
            });
        }
    }

    /// What the e2e reads of the notices: each plugin's text, kind and time left.
    #[allow(dead_code)] // the e2e probe reads it
    pub fn e2e_plugin_notices(&self) -> Value {
        let list: Vec<Value> = self
            .plugin_notices
            .list()
            .iter()
            .map(|n| {
                json!({
                    "plugin": n.plugin,
                    "text": n.text,
                    "kind": if n.error { "error" } else { "info" },
                    "left_ms": n.left_ms,
                    "hovered": n.hovered,
                })
            })
            .collect();
        json!({ "notices": list })
    }
}
