//! The device panel's glue (issue #248, PR 4a): opening and closing it, `UiMsg::Device`, the person's input over the
//! picture and the effects the controller asks of the window (a pick, a panel whose device is gone, an opener).

use std::collections::HashMap;

use flare_surfaces::PanelKind;

use super::*;
use crate::device_ctl::{CHOOSE, DeviceMsg, Effect};

impl AppCtl {
    /// The device panel follows the panels after the web panel did (`sync_panels`).
    pub(super) fn sync_device(&mut self, app: &AppWindow, front: Option<&str>) {
        let names: HashMap<String, String> = self
            .model
            .state
            .tabs
            .iter()
            .map(|t| t.id.clone())
            .zip(self.model.labels())
            .collect();
        let splitting = app.get_panel_splitting();
        let effects = self
            .device
            .update(&self.panels, front, splitting, names.clone());
        let changed = self.apply_device(effects);
        if changed {
            // A dropped panel or a new target: the panels sync again before the device pushes.
            self.panels.sync(app, front);
            let _ = self.device.update(&self.panels, front, splitting, names);
        }
        self.device.push(app);
    }

    /// Does what the device controller asked; true when a panel changed.
    fn apply_device(&mut self, effects: Vec<Effect>) -> bool {
        let mut changed = false;
        for e in effects {
            match e {
                Effect::SetTarget { tab, target } => {
                    if self.panels.set_device_target(&tab, &target) {
                        changed = true;
                        self.panel_changed(&tab);
                    }
                }
                Effect::Drop(tab) => {
                    log::info!("a device panel's device no longer exists: the panel goes");
                    if self.panels.drop_device(&tab) {
                        changed = true;
                        self.panel_changed(&tab);
                    }
                }
                Effect::OpenUrl(url) => (self.deps.open_url)(&url),
                Effect::OpenXcode => {
                    // PR 4b opens the Xcode it picked; the fake source has none.
                    log::info!("device panel: Open Xcode asked (no Xcode with the fake source)");
                }
            }
        }
        changed
    }

    /// Opens the device panel on the tab: on `target` (`sim/<udid>`), or on the device list. False where the kind is
    /// not offered, for a closed tab, or a target that is no device's.
    pub(crate) fn device_open(&mut self, tab: &str, target: Option<&str>) -> bool {
        if !self.device.offered() || !self.is_shell_tab(tab) {
            return false;
        }
        let target = target.unwrap_or(CHOOSE);
        if !self.panels.show(tab, PanelKind::Device, target) {
            return false;
        }
        self.refresh();
        self.panel_changed(tab);
        true
    }

    /// A message for the device controller: its events, a boot's second.
    pub(super) fn device_msg(&mut self, m: DeviceMsg) {
        self.device.on_msg(m);
        self.refresh();
    }

    fn device_after_input(&mut self) {
        let Some(app) = self.app() else { return };
        self.device.push(&app);
    }

    pub(super) fn device_pointer(&mut self, kind: i32, x: f32, y: f32, button: i32, mods: i32) {
        let Some(app) = self.app() else { return };
        if kind == 0 {
            self.panel_focus_seen = true;
        }
        self.device.page_pointer(&app, kind, x, y, button, mods);
        self.device_after_input();
    }

    pub(super) fn device_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        let Some(app) = self.app() else { return };
        self.device.page_wheel(&app, x, y, dx, dy);
        self.device_after_input();
    }

    pub(super) fn device_key(&mut self, down: bool, text: &str, mods: i32, repeat: bool) {
        self.device.page_key(down, text, mods, repeat);
        self.device_after_input();
    }

    pub(super) fn device_act(&mut self, name: &str) {
        let effects = self.device.act(name);
        self.apply_device(effects);
        self.refresh();
    }

    pub(super) fn device_setup(&mut self) {
        let effects = self.device.setup_clicked();
        self.apply_device(effects);
        self.refresh();
    }

    pub(super) fn device_pick(&mut self, i: i32) {
        let effects = self.device.pick(i);
        self.apply_device(effects);
        self.refresh();
    }

    /// The person took over the front device panel (the e2e's stand-in for their press, `DeviceUserInput`).
    pub fn device_user_input(&mut self) {
        if let Some(tab) = self.device.shown().map(str::to_string) {
            self.device.person_input(&tab);
        }
    }

    /// The window is occluded (macOS): the device's stream pauses. Not under `--e2e`, where the parked window may
    /// count as hidden (rule 1117) and the scenario still reads the frames handed to Slint.
    pub fn on_occluded(&mut self, hidden: bool) {
        self.device.set_occluded(hidden && !self.deps.e2e);
        self.refresh();
    }

    /// The `device.state` probe.
    pub fn device_probe(&self) -> serde_json::Value {
        self.device.probe()
    }

    /// The device controller (tests).
    #[cfg(test)]
    pub(crate) fn device_mut(&mut self) -> &mut crate::device_ctl::DeviceCtl {
        &mut self.device
    }
}
