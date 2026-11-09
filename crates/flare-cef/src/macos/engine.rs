//! Starting and stopping CEF in a browser process: the framework is loaded from the bundle
//! (`Contents/Frameworks`), the API version is fixed, the shared application is Flare's `CefAppProtocol` class
//! (headless), and `cef_initialize` runs with windowless rendering and no window of its own. The caller pumps
//! (`do_message_loop_work`) from its own loop; nothing here runs `[NSApp run]`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cef::library_loader::LibraryLoader;
use cef::*;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEventMask};
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use crate::macos::app;
use crate::pin::FRAMEWORK;

/// The switches every Flare browser process starts with, and why. They are the only switches CEF takes:
/// `command_line_args_disabled` keeps the process's own command line out (rule 261), so a switch on Flare's command
/// line (`--remote-debugging-port`, `--no-sandbox`, `--proxy-server`, `--disable-web-security`) never reaches
/// Chromium. Added in [`FlareApp`]'s `on_before_command_line_processing`, which CEF still asks.
pub const SWITCHES: [(&str, &str); 8] = [
    // Chromium's cookie and password encryption asks the login keychain for "Chromium Safe Storage": on a Mac
    // that is a system prompt on the person's screen, and an ad-hoc signed build would ask again after every
    // rebuild. The mock keychain never asks, but its key is a fixed, public one: what a profile on disk keeps
    // (the web panel's cookies, a mod's storage) is only as safe as the file's own permissions until phase 8 gives
    // the signed app a real keychain item (rule 264, an accepted gap). The mini-apps' profiles are in memory.
    ("use-mock-keychain", ""),
    // No crash or error dialog of Chromium's own.
    ("noerrdialogs", ""),
    // No "restore pages?" bubble after a crash.
    ("hide-crash-restore-bubble", ""),
    // The engine calls no one on its own (rule 263). The component updater still registers the on-device model
    // manifest with this switch (measured on CEF 156.0.14: update.googleapis.com, then a download from gvt1.com),
    // so its server is also Flare's dead address: a check is refused on loopback and nothing is fetched.
    ("disable-component-update", ""),
    ("component-updater", COMPONENT_UPDATER),
    // Variations, the network time tracker (clients2.google.com/time) and the other background services.
    ("disable-background-networking", ""),
    ("disable-features", DISABLED_FEATURES),
    // No hyperlink-auditing pings (`<a ping>`).
    ("no-pings", ""),
];

/// The component updater's options: its server is the dead address sealed pages use (`127.0.0.1:9`, where
/// nothing listens), with no pings.
pub const COMPONENT_UPDATER: &str = "url-source=http://127.0.0.1:9/,disable-pings";

/// The Chromium features [`SWITCHES`] turns off (Chromium's names). The first four are the ones CEF turns off itself
/// (a `disable-features` switch replaces CEF's list, so it carries them on); then the network time queries, the AI
/// mode eligibility check (www.google.com/async/folae, sent with the country), the optimization guide's hints and
/// models, and the media router's discovery of devices on the network.
pub const DISABLED_FEATURES: &str = "AutofillActorMode,GlicActorUi,KillOnInvalidNavigationHeaders,LensOverlay,\
NetworkTimeServiceQuerying,AimEnabled,AimServerEligibilityEnabled,AimServerRequestOnStartupEnabled,\
AimUrlNavigationFetchEnabled,OptimizationHints,OptimizationTargetPrediction,OptimizationGuideModelExecution,\
OnDeviceModelBackgroundDownload,MediaRouter,DialMediaRouteProvider";

/// Told when CEF wants its work done: in this many milliseconds (0: now). Called from any thread; the owner queues a
/// pump (the window's `WebEvent::Pump`) and calls [`Engine::work`] on the main thread when it comes.
pub type Schedule = Arc<dyn Fn(i64) + Send + Sync>;

/// Where a browser process keeps its state.
pub struct EngineConfig {
    /// CEF's root cache folder (every profile goes under it). The probe uses a fresh temp folder.
    pub root_cache: PathBuf,
    /// CEF's log, or none. For diagnosis only: with a log file the windowless view never painted (measured
    /// 2026-11-08 on macOS 27.2: 0 paints in 15 s with one, the first paint at about 70 ms without, five runs each).
    pub log_file: Option<PathBuf>,
    /// The external message pump (`external_message_pump`): CEF says when it needs a turn, the owner gives it one.
    /// None: the owner turns it on its own clock ([`Engine::pump`], the engine tests).
    pub schedule: Option<Schedule>,
    /// The app's own `NSApplication` is in charge of the screen (the real Flare window, winit's run loop): the
    /// engine adopts it ([`app::adopt`]) instead of making a headless one, and leaves its activation policy alone.
    pub adopt_app: bool,
    /// Chromium's network log (`--log-net-log`), or none. For the tests: every request the engine made, a page's
    /// or its own, is in it once the engine shut down.
    pub net_log: Option<PathBuf>,
}

/// CEF, started. Dropping it does not stop CEF: call [`Engine::shutdown`] after every browser closed.
pub struct Engine {
    loader: Option<LibraryLoader>,
    pub app: Retained<NSApplication>,
}

wrap_browser_process_handler! {
    struct Process {
        schedule: Option<Schedule>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            if let Some(s) = &self.schedule {
                s(delay_ms.max(0));
            }
        }
    }
}

wrap_app! {
    struct FlareApp {
        schedule: Option<Schedule>,
        net_log: Option<String>,
    }

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Process::new(self.schedule.clone()))
        }

        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cl) = command_line else { return };
            for (name, value) in switches(self.net_log.as_deref()) {
                if value.is_empty() {
                    cl.append_switch(Some(&name.as_str().into()));
                } else {
                    cl.append_switch_with_value(Some(&name.as_str().into()), Some(&value.as_str().into()));
                }
            }
        }
    }
}

/// [`SWITCHES`], with the network log when there is one.
pub fn switches(net_log: Option<&str>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = SWITCHES
        .iter()
        .map(|(n, val)| ((*n).to_string(), (*val).to_string()))
        .collect();
    if let Some(p) = net_log {
        v.push(("log-net-log".into(), p.to_string()));
        v.push(("net-log-capture-mode".into(), "Default".into()));
    }
    v
}

/// The framework the browser process at `exe` loads: `<app>/Contents/Frameworks/<framework>/<binary>`.
pub fn framework_of(exe: &Path) -> Option<PathBuf> {
    let p = exe
        .parent()?
        .join("../Frameworks")
        .join(FRAMEWORK)
        .join("Chromium Embedded Framework");
    p.exists().then_some(p)
}

impl Engine {
    /// Loads and initializes CEF in this (the browser) process, on the main thread. Fails with a sentence when the
    /// process is not inside a bundle with the framework, when `NSApp` is someone else's, or when CEF refuses.
    pub fn start(cfg: &EngineConfig) -> Result<Engine, String> {
        let mtm = MainThreadMarker::new().ok_or("CEF starts on the main thread")?;
        let exe = std::env::current_exe().map_err(|e| format!("no executable path: {e}"))?;
        if framework_of(&exe).is_none() {
            return Err(format!(
                "{} is not inside a bundle with {FRAMEWORK} in Contents/Frameworks",
                exe.display()
            ));
        }
        let loader = LibraryLoader::new(&exe, false);
        if !loader.load() {
            return Err("the CEF framework did not load".to_string());
        }
        // Fixes the API version the bindings were generated for (must come before any other CEF call).
        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
        let app = if cfg.adopt_app {
            app::adopt(mtm)?
        } else {
            app::install(mtm)?
        };
        app::ensure_protocols(&app);

        let args = args::Args::new();
        let net_log = cfg
            .net_log
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let mut cef_app = FlareApp::new(cfg.schedule.clone(), net_log);
        // A browser process: CEF answers -1 (a child process type would run and exit here).
        let code = execute_process(
            Some(args.as_main_args()),
            Some(&mut cef_app),
            std::ptr::null_mut(),
        );
        if code >= 0 {
            return Err(format!(
                "this executable was started as a CEF child process (exit {code}); helpers run flare-cef-helper"
            ));
        }
        let root = cfg.root_cache.to_string_lossy().into_owned();
        let log = cfg
            .log_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let settings = Settings {
            // The sandbox stays on: every helper enters it (`cef::sandbox::Sandbox`) before CEF runs there.
            no_sandbox: 0,
            windowless_rendering_enabled: 1,
            // Only Flare's own switches ([`SWITCHES`]): the process's command line is not CEF's to read (rule 261).
            command_line_args_disabled: 1,
            // The caller's loop calls `do_message_loop_work`: on its own clock, or when CEF asks (`schedule`).
            external_message_pump: i32::from(cfg.schedule.is_some()),
            multi_threaded_message_loop: 0,
            root_cache_path: root.as_str().into(),
            // An empty cache path is an in-memory profile.
            cache_path: "".into(),
            persist_session_cookies: 0,
            log_file: log.as_str().into(),
            log_severity: if log.is_empty() {
                LogSeverity::DISABLE
            } else {
                LogSeverity::WARNING
            },
            ..Default::default()
        };
        if initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut cef_app),
            std::ptr::null_mut(),
        ) != 1
        {
            return Err(format!(
                "cef_initialize failed (exit code {})",
                get_exit_code()
            ));
        }
        Ok(Engine {
            loader: Some(loader),
            app,
        })
    }

    /// One turn of the main thread's work, never blocking: AppKit's queued events (through `sendEvent:`, where
    /// CEF wants them), the run loop's due sources and timers (Chromium posts its compositor's frames and the main
    /// dispatch queue's tasks there: without this turn a windowless view never paints), then CEF's own work.
    pub fn pump(&self) {
        // SAFETY: a static NSString AppKit exports.
        let mode = unsafe { NSDefaultRunLoopMode };
        let past = NSDate::distantPast();
        while let Some(ev) = self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&past),
            mode,
            true,
        ) {
            self.app.sendEvent(&ev);
        }
        // SAFETY: a static CFString CoreFoundation exports.
        let cf_mode = unsafe { kCFRunLoopDefaultMode };
        CFRunLoop::run_in_mode(cf_mode, 0.0, false);
        do_message_loop_work();
    }

    /// CEF's own work only (`do_message_loop_work`), for a process whose run loop is someone else's (the real
    /// app: winit runs AppKit's loop, so its events and the run loop's sources are already served).
    pub fn work(&self) {
        do_message_loop_work();
    }

    /// Pumps for `ms` milliseconds.
    pub fn pump_for(&self, ms: u64) {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            self.pump();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Pumps until `done` holds or `ms` pass; whether it held.
    pub fn pump_until(&self, ms: u64, mut done: impl FnMut() -> bool) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            self.pump();
            if done() {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Stops CEF (every browser must be closed) and unloads the framework.
    pub fn shutdown(mut self) {
        shutdown();
        drop(self.loader.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_is_started_with_its_quiet_switches_and_keeps_cefs_own_disabled_features() {
        let names: Vec<&str> = SWITCHES.iter().map(|(n, _)| *n).collect();
        for n in [
            "disable-component-update",
            "component-updater",
            "disable-background-networking",
            "disable-features",
            "use-mock-keychain",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        assert!(COMPONENT_UPDATER.starts_with("url-source=http://127.0.0.1:9/"));
        for f in [
            "AutofillActorMode",
            "GlicActorUi",
            "KillOnInvalidNavigationHeaders",
            "LensOverlay",
            "NetworkTimeServiceQuerying",
            "AimServerEligibilityEnabled",
        ] {
            assert!(DISABLED_FEATURES.split(',').any(|x| x == f), "{f}");
        }
        assert!(!DISABLED_FEATURES.contains(char::is_whitespace));
        let with_log = switches(Some("/tmp/n.json"));
        assert!(with_log.contains(&("log-net-log".to_string(), "/tmp/n.json".to_string())));
        assert_eq!(switches(None).len(), SWITCHES.len());
    }
}
