//! Starting and stopping CEF in a browser process: the framework is loaded from the bundle
//! (`Contents/Frameworks`), the API version is fixed, the shared application is Flare's `CefAppProtocol` class
//! (headless), and `cef_initialize` runs with windowless rendering and no window of its own. The caller pumps
//! (`do_message_loop_work`) from its own loop; nothing here runs `[NSApp run]`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cef::library_loader::LibraryLoader;
use cef::*;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEventMask};
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use crate::macos::{app, helpers};
use crate::pin::FRAMEWORK;

/// The switches every Flare browser process starts with, and why. They are the only switches CEF takes:
/// `command_line_args_disabled` keeps the process's own command line out (rule 1030), so a switch on Flare's command
/// line (`--remote-debugging-port`, `--no-sandbox`, `--proxy-server`, `--disable-web-security`) never reaches
/// Chromium. Added in [`FlareApp`]'s `on_before_command_line_processing`, which CEF still asks.
pub const SWITCHES: [(&str, &str); 8] = [
    // Chromium's cookie and password encryption asks the login keychain for its key: an ad-hoc signed build (a dev
    // bundle, a test bundle) would be asked again after every rebuild, a system prompt on the person's screen. So
    // such a build uses the mock keychain, which never asks but whose key is a fixed, public one: what a profile on
    // disk keeps (the web panel's cookies, a mod's storage) is then only as safe as the file's permissions (rule
    // 1033). The real Flare window in a Flare.app signed by Flare's Developer ID team drops this switch and keeps its
    // key in a keychain item of its own ([`keychain_for`], [`KEYCHAIN_SERVICE`], rule 1045). The mini-apps'
    // profiles are in memory.
    ("use-mock-keychain", ""),
    // No crash or error dialog of Chromium's own.
    ("noerrdialogs", ""),
    // No "restore pages?" bubble after a crash.
    ("hide-crash-restore-bubble", ""),
    // The engine calls no one on its own (rule 1032). The component updater still registers the on-device model
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

/// The keychain item (service and account) that holds the key of the profiles on disk, in the signed app: Flare's
/// own, never CEF's default "Chromium Safe Storage", which Chromium itself and every unbranded CEF app share (an
/// item another app made would be a keychain prompt naming that app's item).
pub const KEYCHAIN_SERVICE: &str = "Flare Safe Storage";
pub const KEYCHAIN_ACCOUNT: &str = "Flare";

/// What the running code must satisfy before the engine uses a real keychain item: Flare's identifier, signed by a
/// Developer ID Application certificate Apple issued to Flare's team. The two OIDs are Apple's: the Developer ID
/// intermediate ([`DEVELOPER_ID_CA_OID`]) and the Developer ID Application leaf ([`DEVELOPER_ID_LEAF_OID`]); without
/// them an Apple Development or Mac App Distribution signature of the same team would pass too, and make the item
/// with an access list the Developer ID app would be asked about. An ad-hoc signature has no such leaf, so a dev or
/// test bundle is never asked to touch the keychain.
pub const DEVELOPER_ID_REQUIREMENT: &str = "identifier \"com.flare.terminal\" and anchor apple generic and \
certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and \
certificate leaf[subject.OU] = \"WV7BY8CUM8\"";

/// The extension Apple puts on the Developer ID Certification Authority (the intermediate).
pub const DEVELOPER_ID_CA_OID: &str = "1.2.840.113635.100.6.2.6";
/// The extension Apple puts on a Developer ID Application certificate (the leaf).
pub const DEVELOPER_ID_LEAF_OID: &str = "1.2.840.113635.100.6.1.13";

/// Where Chromium's key for the profiles on disk lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keychain {
    /// `use-mock-keychain`: never asks, a fixed public key (rule 1033).
    Mock,
    /// Flare's own item ([`KEYCHAIN_SERVICE`]), made by the signed app on its first run; its access is tied to the
    /// Developer ID signature, so an update keeps it without asking.
    Own,
}

/// The keychain the engine uses: its own item only for the real Flare window (`adopt_app`) in a bundle signed by
/// Flare's Developer ID team; the mock everywhere else (tests, dev bundles, ad-hoc builds).
pub fn keychain_for(adopt_app: bool, developer_id_signed: bool) -> Keychain {
    if adopt_app && developer_id_signed {
        Keychain::Own
    } else {
        Keychain::Mock
    }
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySelf(flags: u32, code: *mut *mut std::ffi::c_void) -> i32;
    fn SecCodeCheckValidity(
        code: *mut std::ffi::c_void,
        flags: u32,
        requirement: *mut std::ffi::c_void,
    ) -> i32;
    fn SecRequirementCreateWithString(
        text: *const std::ffi::c_void,
        flags: u32,
        req: *mut *mut std::ffi::c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// Whether this process is Flare signed by its Developer ID team ([`DEVELOPER_ID_REQUIREMENT`]). Reads the running
/// code's signature only: no keychain, no prompt.
pub fn signed_by_flares_team() -> bool {
    let text = objc2_foundation::NSString::from_str(DEVELOPER_ID_REQUIREMENT);
    let mut req = std::ptr::null_mut();
    let mut code = std::ptr::null_mut();
    // SAFETY: an NSString is a CFString (toll-free bridged); both out pointers are valid, and what they get is
    // released below.
    unsafe {
        if SecRequirementCreateWithString(Retained::as_ptr(&text).cast(), 0, &mut req) != 0
            || req.is_null()
        {
            return false;
        }
        let ok = SecCodeCopySelf(0, &mut code) == 0
            && !code.is_null()
            && SecCodeCheckValidity(code, 0, req) == 0;
        if !code.is_null() {
            CFRelease(code);
        }
        CFRelease(req);
        ok
    }
}

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

/// CEF, started. Dropping it does not stop CEF: call [`Engine::shutdown`] after every browser closed. A drop without
/// it keeps the framework loaded (CEF still runs in the process, rule 1156).
pub struct Engine {
    loader: Option<LibraryLoader>,
    pub app: Retained<NSApplication>,
    profile_key_note: Option<String>,
    /// The GPU and utility helpers' ends ([`helpers`]), none when the kernel gave no queue.
    helpers: Option<helpers::Watcher>,
    helper_exits: Arc<Mutex<Vec<helpers::HelperExit>>>,
}

wrap_browser_process_handler! {
    struct Process {
        schedule: Option<Schedule>,
        hint: Option<helpers::Hint>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            if let Some(s) = &self.schedule {
                s(delay_ms.max(0));
            }
        }

        fn on_before_child_process_launch(&self, _command_line: Option<&mut CommandLine>) {
            if let Some(h) = &self.hint {
                h.launching();
            }
        }
    }
}

wrap_app! {
    struct FlareApp {
        schedule: Option<Schedule>,
        net_log: Option<String>,
        keychain: Keychain,
        hint: Option<helpers::Hint>,
    }

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Process::new(self.schedule.clone(), self.hint.clone()))
        }

        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cl) = command_line else { return };
            for (name, value) in switches_for(self.net_log.as_deref(), self.keychain) {
                if value.is_empty() {
                    cl.append_switch(Some(&name.as_str().into()));
                } else {
                    cl.append_switch_with_value(Some(&name.as_str().into()), Some(&value.as_str().into()));
                }
            }
        }
    }
}

/// [`SWITCHES`], with the network log when there is one (the mock keychain).
pub fn switches(net_log: Option<&str>) -> Vec<(String, String)> {
    switches_for(net_log, Keychain::Mock)
}

/// [`SWITCHES`] for `keychain`: [`Keychain::Own`] leaves out `use-mock-keychain`, nothing else.
pub fn switches_for(net_log: Option<&str>, keychain: Keychain) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = SWITCHES
        .iter()
        .filter(|(n, _)| keychain == Keychain::Mock || *n != "use-mock-keychain")
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

/// One turn of the main thread's run loop, never blocking and without CEF's own work: AppKit's queued events
/// (through `sendEvent:`) and the run loop's due sources and timers. What winit's loop does for the real app, so a
/// test can run the app's way: CEF's work only when CEF asks for it.
pub fn run_loop_turn(app: &NSApplication) {
    // SAFETY: a static NSString AppKit exports.
    let mode = unsafe { NSDefaultRunLoopMode };
    let past = NSDate::distantPast();
    while let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(&past),
        mode,
        true,
    ) {
        app.sendEvent(&ev);
    }
    // SAFETY: a static CFString CoreFoundation exports.
    let cf_mode = unsafe { kCFRunLoopDefaultMode };
    CFRunLoop::run_in_mode(cf_mode, 0.0, false);
}

/// [`run_loop_turn`] of the shared application.
pub fn shared_run_loop_turn(mtm: MainThreadMarker) {
    run_loop_turn(&NSApplication::sharedApplication(mtm));
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
        // The signature is read only for the real window: a test or a probe never looks at the keychain.
        let keychain = keychain_for(cfg.adopt_app, cfg.adopt_app && signed_by_flares_team());
        // A GPU or utility helper that fails is queued for the owner ([`Engine::helper_exits`]) and the owner is
        // asked for a turn, as CEF asks.
        let helper_exits: Arc<Mutex<Vec<helpers::HelperExit>>> = Arc::default();
        let (exits, schedule) = (Arc::clone(&helper_exits), cfg.schedule.clone());
        let watcher = helpers::Watcher::start(Arc::new(move |e| {
            exits.lock().unwrap_or_else(|e| e.into_inner()).push(e);
            if let Some(s) = &schedule {
                s(0);
            }
        }));
        let hint = watcher.as_ref().map(helpers::Watcher::hint);
        let mut cef_app = FlareApp::new(cfg.schedule.clone(), net_log, keychain, hint);
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
        // The profiles on disk were written with one key; a switch between the mock and Flare's own item sets the
        // old key's cookies aside instead of letting Chromium drop them (rule 1051).
        let profile_key_note = crate::macos::profile_key::settle(&cfg.root_cache, keychain);
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
            // Only Flare's own switches ([`SWITCHES`]): the process's command line is not CEF's to read (rule 1030).
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
            // Flare's own keychain item, never Chromium's shared one (read only without the mock keychain).
            keychain_service_name: KEYCHAIN_SERVICE.into(),
            keychain_account_name: KEYCHAIN_ACCOUNT.into(),
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
        crate::macos::profile_key::mark(&cfg.root_cache, keychain);
        Ok(Engine {
            loader: Some(loader),
            app,
            profile_key_note,
            helpers: watcher,
            helper_exits,
        })
    }

    /// What [`crate::macos::profile_key::settle`] did when the profiles' key changed at this start (for the log).
    pub fn profile_key_note(&self) -> Option<&str> {
        self.profile_key_note.as_deref()
    }

    /// One turn of the main thread's work, never blocking: AppKit's queued events (through `sendEvent:`, where
    /// CEF wants them), the run loop's due sources and timers (Chromium posts its compositor's frames and the main
    /// dispatch queue's tasks there: without this turn a windowless view never paints), then CEF's own work.
    pub fn pump(&self) {
        run_loop_turn(&self.app);
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

    /// The GPU and utility helpers that failed since the last call (Chromium starts them again by itself).
    pub fn helper_exits(&self) -> Vec<helpers::HelperExit> {
        std::mem::take(&mut *self.helper_exits.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Stops CEF (every browser must be closed) and unloads the framework.
    pub fn shutdown(mut self) {
        // The helpers CEF ends now are not failures.
        if let Some(w) = &self.helpers {
            w.ending();
        }
        shutdown();
        drop(self.helpers.take());
        drop(self.loader.take());
    }
}

impl Drop for Engine {
    /// Without [`Engine::shutdown`] CEF is still initialized: the framework stays loaded (unloading it under a live
    /// CEF is what the idle web host did, rule 1156).
    fn drop(&mut self) {
        if let Some(loader) = self.loader.take() {
            std::mem::forget(loader);
        }
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

    #[test]
    fn only_the_real_window_of_the_developer_id_app_uses_a_keychain_item_of_its_own() {
        assert_eq!(keychain_for(true, true), Keychain::Own);
        for (adopt, signed) in [(true, false), (false, true), (false, false)] {
            assert_eq!(
                keychain_for(adopt, signed),
                Keychain::Mock,
                "{adopt} {signed}"
            );
        }
        let mock = switches_for(None, Keychain::Mock);
        let own = switches_for(None, Keychain::Own);
        assert!(mock.iter().any(|(n, _)| n == "use-mock-keychain"));
        assert!(!own.iter().any(|(n, _)| n == "use-mock-keychain"));
        assert_eq!(own.len() + 1, mock.len(), "nothing else changes");
        // Flare's own item, never the one Chromium and other CEF apps share.
        assert_eq!(KEYCHAIN_SERVICE, "Flare Safe Storage");
        assert_ne!(KEYCHAIN_SERVICE, "Chromium Safe Storage");
        assert!(DEVELOPER_ID_REQUIREMENT.contains(crate::macos::sign::TEAM));
        assert!(DEVELOPER_ID_REQUIREMENT.contains(crate::macos::release::BUNDLE_ID));
        // A Developer ID Application certificate only: the intermediate and the leaf, by Apple's OIDs (an Apple
        // Development or Mac App Distribution signature of the same team must not pass).
        assert!(DEVELOPER_ID_REQUIREMENT.contains(&format!(
            "certificate 1[field.{DEVELOPER_ID_CA_OID}] exists"
        )));
        assert!(DEVELOPER_ID_REQUIREMENT.contains(&format!(
            "certificate leaf[field.{DEVELOPER_ID_LEAF_OID}] exists"
        )));
        // A test binary carries the linker's ad-hoc signature: it is never Flare's Developer ID, so a test run never
        // reaches the keychain.
        assert!(!signed_by_flares_team());
    }
}
