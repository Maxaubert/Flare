//! The device panel's glue (issue #248, PR 4a and 4b): opening and closing it, `UiMsg::Device`, the person's input over
//! the picture, the effects the controller asks of the window (a pick, a panel whose device is gone, an opener), the
//! real simulator's install (the Xcode pick on a worker, the sim host made by the first device panel), the tab menu's
//! entry and the shutdown at quit.

use std::collections::HashMap;

use flare_surfaces::PanelKind;

use super::*;
use crate::device_ctl::{CHOOSE, DeviceMsg, Effect, Woken};

/// The tab menu's row that opens the device panel (macOS on Apple silicon with Xcode, or the fake under `--e2e`).
pub const DEVICE_MENU_ROW: &str = "Device panel";

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
                Effect::OpenXcode => match self.device.xcode_app() {
                    // The Xcode the pick chose, through Launch Services (`open`, never Simulator.app).
                    Some(app) if !self.deps.e2e => {
                        let mut c = std::process::Command::new("/usr/bin/open"); // sync-ok: spawned detached
                        c.arg(app);
                        crate::device_ctl::spawn_detached(c);
                    }
                    _ => log::info!("device panel: Open Xcode asked (no Xcode to open here)"),
                },
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

    /// A message for the device controller: its events, a boot's second, the Xcode pick. A ring frame alone only asks
    /// for a redraw (the notifier draws it, rule 1311); a change of the panel refreshes the window.
    pub(super) fn device_msg(&mut self, m: DeviceMsg) {
        match self.device.on_msg(m) {
            Woken::Refresh => self.refresh(),
            Woken::Redraw => {
                if let Some(app) = self.app() {
                    app.window().request_redraw();
                }
            }
            Woken::Nothing => {}
        }
    }

    /// The device panel's ring frames, for the render notifier's weak slot.
    pub fn device_frames(
        &self,
    ) -> std::rc::Rc<std::cell::RefCell<crate::device_frames::DeviceFrames>> {
        self.device.frames()
    }

    /// The real simulator (macOS on Apple silicon; under `--e2e` only for the opt-in `deviceReal`, which sets
    /// `FLARE_DEVICE_REAL=1`): the Xcode pick and `booted.json` are read on a worker, and the sim host is made by the
    /// first device panel. Its events wake the window through one coalesced `UiMsg::Device(Wake)`.
    #[cfg(target_os = "macos")]
    pub fn install_device(&mut self, profile: &std::path::Path) {
        use std::sync::Arc;

        use flare_device::client::{SimConfig, SimSource};

        use crate::device_ctl::{Booted, Started, Starter, XcodeAnswer, booted_file};

        if !crate::device_ctl::real_platform() {
            return;
        }
        if self.deps.e2e && std::env::var_os("FLARE_DEVICE_REAL").is_none() {
            return;
        }
        let exe = std::env::current_exe() // sync-ok: startup, before the window shows
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(flare_win::names::SIM_HOST_EXE)))
            .unwrap_or_default();
        let log = profile.join("logs").join("sim-host.log");
        let waker = self.device.waker();
        let starter: Starter = Box::new(move |x| {
            let mut cfg = SimConfig::new(exe.clone(), x.developer_dir(), log.clone());
            cfg.on_event = Some(Arc::clone(&waker));
            let sim = SimSource::start(cfg);
            let ring: Arc<dyn crate::device_frames::RingSource> = Arc::new(sim.frames());
            Started {
                source: Box::new(sim),
                ring: Some(ring),
            }
        });
        let booted = booted_file(profile);
        self.device.install_real(
            starter,
            Some(booted.clone()),
            std::rc::Rc::new(|dev_dir, udids| {
                crate::device_ctl::spawn_detached(crate::device_ctl::shutdown_command(
                    dev_dir, udids,
                ));
            }),
        );
        self.panels.set_device_offered(true);
        let post = Arc::clone(&self.deps.post);
        std::thread::spawn(move || {
            let answer = XcodeAnswer {
                xcode: flare_device::xcode::find(),
                booted: Booted::load(&booted),
            };
            post(UiMsg::Device(DeviceMsg::Xcode(answer)));
        });
    }

    /// The tab menu offers the device panel (macOS on Apple silicon with Xcode, or the fake under `--e2e`).
    pub(super) fn device_menu_offered(&self, tab: &str) -> bool {
        self.device.menu_offered() && self.is_shell_tab(tab)
    }

    /// Quit (rule 1319): the sim host goes, and the simulators Flare booted are shut down, detached, when the setting
    /// is on. Quit never waits for them.
    pub(super) fn device_quit(&mut self) {
        let on = flare_settings::agent_prefs::device_shutdown_on_quit(&self.prefs);
        self.device.quit(on);
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
