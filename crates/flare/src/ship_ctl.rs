//! M9 Task 32: ShipCtl, the Velopack bootstrap, the verb, the update modes and the window icon.
//!
//! Everything here that has a rule is a plain function over plain values (`hook_root`, `Ship`,
//! `choose_feed`), so it is tested without a window, a registry or a network. The window glue
//! (`window.rs`) only calls these.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};

use flare_update::engine::{CondvarSleeper, Engine, Events, Mode, mode_for};
use flare_update::feed::{Feed, NullFeed, RefusingFeed};
use flare_update::preview::SleepWaiter;
use flare_update::types::Offer;
use flare_update::velopack_feed::{self, Hooks};
#[cfg(feature = "live-updates")]
use flare_win::names::REPO_URL;
use flare_win::names::{AUMID, E2E_REG_PREFIX};
use flare_win::packaged;
use flare_win::registry::{Registry, WinRegistry};
use flare_win::verb::remove_verb;
use flare_win::verb_keys::DEFAULT_CLASSES_ROOT;
use flare_win::verb_switch::{VerbSwitch, off_marker_path};

use crate::e2e;

/// The version the Velopack first-run hook reported, if this launch is the first after an install.
/// Task 31 (the diagnostics log) writes the `first-run` crumb from it; `bootstrap()` fills it from
/// the hook.
pub static FIRST_RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The id `rc_text` gives the exe's icon resource (M9 Task 26).
const ICON_RESOURCE_ID: usize = 1;

/// A `FLARE_E2E_<suffix>` variable: present only under `--e2e`. The ONE place this crate reads
/// them (besides `e2e.rs` itself).
pub fn e2e_var(suffix: &str) -> Option<String> {
    e2e::global().env(&["FLARE_E2E", "_", suffix].concat())
}

// ---- the bootstrap -------------------------------------------------------------------------------

/// The first statement of `main`: the e2e flag, Velopack's hooks (they may end the process), the
/// taskbar identity.
pub fn bootstrap() {
    e2e::init(std::env::args().any(|a| a == "--e2e"));
    velopack_feed::bootstrap(Hooks {
        before_uninstall: Box::new(|| {
            if let Some(root) = classes_root_for_hook() {
                uninstall_hook(&WinRegistry::current_user(), &root);
            }
        }),
        first_run: Box::new(|v| {
            let _ = FIRST_RUN.set(v);
        }),
    });
    set_aumid();
}

/// The classes root the uninstall hook cleans (pure form of [`classes_root_for_hook`]).
///
/// Without `--e2e`: the real root. Under `--e2e`: only a sandbox root under `Software\FlareE2E\`
/// without a `..`; anything else is `None` and the hook does nothing (a test run can never delete
/// the verb of a Flare the owner installed by hand).
pub fn hook_root(e2e: bool, verb_root: Option<&str>) -> Option<String> {
    if !e2e {
        return Some(DEFAULT_CLASSES_ROOT.to_string());
    }
    let v = verb_root?.trim_end_matches('\\');
    if v.contains("..") {
        return None;
    }
    let prefix = E2E_REG_PREFIX;
    let starts = v.len() > prefix.len()
        && v.get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix));
    starts.then(|| format!("{v}\\Classes"))
}

/// [`hook_root`] over the process: the e2e flag and `FLARE_E2E_VERB_ROOT`.
pub fn classes_root_for_hook() -> Option<String> {
    hook_root(e2e::global().on(), e2e_var("VERB_ROOT").as_deref())
}

/// Velopack's before-uninstall hook: removes the verb keys and nothing else (the profile, the
/// marker and the models are the user's).
pub fn uninstall_hook(reg: &dyn Registry, root: &str) {
    remove_verb(reg, root);
}

/// Sets the taskbar identity in every build, so a dev run and an installed run share it.
pub fn set_aumid() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let wide: Vec<u16> = AUMID.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; the call copies it.
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
}

// ---- what this run is ----------------------------------------------------------------------------

/// What this run is, built once: installed or not, e2e or not, where the verb writes.
#[derive(Clone, Debug)]
#[allow(dead_code)] // `preview` and `exe` are read by the `live-updates` build and the tests
pub struct Ship {
    pub packaged: bool,
    pub e2e: bool,
    pub preview: bool,
    pub exe: PathBuf,
    pub verb_exe: Option<String>,
    pub mode: Mode,
    pub root: String,
    pub sandboxed: bool,
}

impl Ship {
    /// This process: the exe, the flags and the e2e seams.
    pub fn detect(exe: PathBuf, e2e: bool, preview: bool) -> Ship {
        let packaged = packaged::is_packaged(&exe);
        Ship::from_parts(
            exe,
            packaged,
            e2e,
            preview,
            cfg!(feature = "live-updates"),
            e2e_var("VERB_ROOT").as_deref(),
            e2e_var("VERB_EXE").as_deref(),
        )
    }

    /// The pure form of [`Ship::detect`].
    pub fn from_parts(
        exe: PathBuf,
        packaged: bool,
        e2e: bool,
        preview: bool,
        live_feature: bool,
        verb_root: Option<&str>,
        verb_exe_seam: Option<&str>,
    ) -> Ship {
        let sandbox_root = if e2e { hook_root(e2e, verb_root) } else { None };
        let sandboxed = sandbox_root.is_some();
        let root = sandbox_root.unwrap_or_else(|| DEFAULT_CLASSES_ROOT.to_string());
        let verb_exe = if sandboxed {
            Some(verb_exe_seam.map_or_else(|| exe.to_string_lossy().into_owned(), str::to_string))
        } else if packaged {
            packaged::verb_exe(&exe)
        } else {
            None
        };
        Ship {
            packaged,
            e2e,
            preview,
            exe,
            verb_exe,
            mode: mode_for(packaged, e2e, preview, live_feature),
            root,
            sandboxed,
        }
    }

    /// Whether any verb write is allowed: the installed app, or a sandboxed e2e run. An e2e run
    /// without a sandbox root never writes, so the real `Software\Classes` is never touched.
    pub fn allowed(&self) -> bool {
        (self.packaged && !self.e2e) || self.sandboxed
    }

    /// The verb switch over `reg`, with the off marker in `profile`.
    pub fn verb_switch(&self, reg: Arc<dyn Registry>, profile: &Path) -> Arc<VerbSwitch> {
        Arc::new(VerbSwitch::new(
            reg,
            self.root.clone(),
            self.allowed(),
            self.verb_exe.clone(),
            off_marker_path(profile),
        ))
    }

    /// The real switch (current user hive).
    pub fn real_verb_switch(&self, profile: &Path) -> Arc<VerbSwitch> {
        self.verb_switch(Arc::new(WinRegistry::current_user()), profile)
    }
}

// ---- the engine ----------------------------------------------------------------------------------

/// The feed, the mode and the offer to show at once.
pub struct FeedChoice {
    pub feed: Arc<dyn Feed>,
    pub mode: Mode,
    /// Set under `--e2e` with `FLARE_E2E_UPDATE_OFFER`: shown after `engine.start()` through
    /// `engine.set_offer`, so the chip appears with `checks` still 0.
    pub preset_offer: Option<Offer>,
}

/// Picks the feed (D11 and the e2e seams). `env` reads a `FLARE_E2E_<suffix>` variable.
pub fn choose_feed(ship: &Ship, env: &dyn Fn(&str) -> Option<String>) -> FeedChoice {
    if ship.e2e && env("UPDATE_OFFER").is_some() {
        let notes = env("UPDATE_NOTES").unwrap_or_default();
        return FeedChoice {
            feed: Arc::new(RefusingFeed::new(&notes)),
            mode: ship.mode,
            preset_offer: Some(Offer {
                version: "99.0.0".into(),
                notes,
                mock: false,
                token: 0,
            }),
        };
    }
    #[cfg(feature = "live-updates")]
    {
        use flare_update::velopack_feed::VelopackFeed;
        if ship.e2e
            && ship.packaged
            && let Some(value) = env("UPDATE_SOURCE")
        {
            // Task 38's seams: a loopback feed or a folder of packages; the check runs once at launch.
            // A URL goes to `loopback`, which refuses every host but 127.0.0.1 itself.
            let feed = if value.contains("://") {
                VelopackFeed::loopback(&value)
            } else {
                VelopackFeed::with_source(
                    Box::new(velopack::sources::FileSource::new(&value)),
                    None,
                )
            };
            if let Ok(feed) = feed {
                return FeedChoice {
                    feed: Arc::new(feed),
                    mode: mode_for(ship.packaged, false, ship.preview, true),
                    preset_offer: None,
                };
            }
        }
        if ship.mode == Mode::Live
            && let Ok(feed) = VelopackFeed::github(REPO_URL)
        {
            return FeedChoice {
                feed: Arc::new(feed),
                mode: Mode::Live,
                preset_offer: None,
            };
        }
    }
    FeedChoice {
        feed: Arc::new(NullFeed),
        mode: ship.mode,
        preset_offer: None,
    }
}

/// The engine for this run. `events` is `PostEvents` in production.
pub fn build_engine(
    choice: &FeedChoice,
    current: &str,
    user_data_dir: Option<PathBuf>,
    events: Arc<dyn Events>,
) -> Arc<Engine> {
    Engine::new(
        choice.mode,
        Arc::clone(&choice.feed),
        current,
        user_data_dir,
        events,
        Arc::new(CondvarSleeper::new()),
        Arc::new(SleepWaiter),
    )
}

// ---- the window icon -----------------------------------------------------------------------------

static BIG_ICON: AtomicIsize = AtomicIsize::new(0);
static SMALL_ICON: AtomicIsize = AtomicIsize::new(0);

/// Puts the exe's own icon (resource 1) on the window at its monitor's DPI. A null handle (a test
/// binary without the resource) is ignored. The handles are shared system-cached icons, kept for
/// the process lifetime and never destroyed.
pub fn set_window_icon(hwnd: isize) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, SM_CXICON,
        SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
    if hwnd == 0 {
        return;
    }
    // SAFETY: plain Win32 calls; `hwnd` is a window of this process, the module handle is this
    // exe's, and the icon id is passed as an integer resource (MAKEINTRESOURCE).
    unsafe {
        let dpi = match GetDpiForWindow(hwnd as _) {
            0 => 96,
            d => d,
        };
        let module = GetModuleHandleW(std::ptr::null());
        let load = |cx: i32, cy: i32| {
            LoadImageW(
                module,
                ICON_RESOURCE_ID as *const u16,
                IMAGE_ICON,
                cx,
                cy,
                LR_DEFAULTCOLOR | LR_SHARED,
            )
        };
        let big = load(
            GetSystemMetricsForDpi(SM_CXICON, dpi),
            GetSystemMetricsForDpi(SM_CYICON, dpi),
        );
        let small = load(
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CYSMICON, dpi),
        );
        if !big.is_null() {
            BIG_ICON.store(big as isize, Ordering::SeqCst);
            SendMessageW(hwnd as _, WM_SETICON, ICON_BIG as usize, big as isize);
        }
        if !small.is_null() {
            SMALL_ICON.store(small as isize, Ordering::SeqCst);
            SendMessageW(hwnd as _, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_win::registry::FakeRegistry;
    use flare_win::verb::install_verb;
    use std::collections::HashMap;

    fn src(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name);
        std::fs::read_to_string(p).unwrap() // sync-ok: test code only
    }

    fn code_of(text: &str) -> &str {
        text.split("#[cfg(test)]").next().unwrap_or("")
    }

    fn ship(packaged: bool, e2e: bool, preview: bool, live: bool, root: Option<&str>) -> Ship {
        Ship::from_parts(
            PathBuf::from(r"C:\Flare\current\Flare.exe"),
            packaged,
            e2e,
            preview,
            live,
            root,
            Some(r"C:\Flare\current\Flare.exe"),
        )
    }

    #[test]
    fn the_first_statement_of_main_is_the_bootstrap() {
        let text = src("main.rs");
        let body = text.split("fn main() {").nth(1).expect("fn main");
        let first = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("use "))
            .unwrap();
        assert_eq!(first, "ship_ctl::bootstrap();");
    }

    #[test]
    fn the_uninstall_hook_removes_every_key_the_app_writes() {
        let reg = FakeRegistry::new();
        assert!(install_verb(
            &reg,
            DEFAULT_CLASSES_ROOT,
            r"C:\Flare\current\Flare.exe"
        ));
        assert!(!reg.keys().is_empty());
        uninstall_hook(&reg, DEFAULT_CLASSES_ROOT);
        let left: Vec<String> = reg
            .keys()
            .into_iter()
            .filter(|k| k.to_lowercase().starts_with(r"software\classes\directory"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn the_uninstall_hook_leaves_a_neighbour_alone() {
        let reg = FakeRegistry::new();
        let other = ["Pri", "smTerminal"].concat();
        let other2 = ["OpenWith", "Pri", "sm"].concat();
        let a = format!(r"Software\Classes\Directory\shell\{other}");
        let b = format!(r"Software\Classes\Directory\shell\{other2}");
        reg.set_string(&a, None, "Open in it").unwrap();
        reg.set_string(&b, None, "Open with it").unwrap();
        install_verb(&reg, DEFAULT_CLASSES_ROOT, r"C:\Flare\current\Flare.exe");
        uninstall_hook(&reg, DEFAULT_CLASSES_ROOT);
        let keys: Vec<String> = reg.keys().iter().map(|k| k.to_lowercase()).collect();
        assert!(keys.contains(&a.to_lowercase()), "{keys:?}");
        assert!(keys.contains(&b.to_lowercase()), "{keys:?}");
    }

    #[test]
    fn the_hook_reads_the_sandbox_root_only_under_e2e_and_only_for_flare_e2e_keys() {
        let real = Some(DEFAULT_CLASSES_ROOT.to_string());
        assert_eq!(hook_root(false, Some(r"Software\Classes")), real);
        assert_eq!(hook_root(false, Some(r"Software\FlareE2E\x")), real);
        assert_eq!(hook_root(false, None), real);
        assert_eq!(hook_root(true, Some(r"Software\Classes")), None);
        assert_eq!(hook_root(true, None), None);
        assert_eq!(hook_root(true, Some(r"Software\FlareE2E\..\Classes")), None);
        assert_eq!(hook_root(true, Some(r"Software\FlareE2E\")), None);
        assert_eq!(
            hook_root(true, Some(r"Software\FlareE2E\x")),
            Some(r"Software\FlareE2E\x\Classes".to_string())
        );
        assert_eq!(
            hook_root(true, Some(r"software\flaree2e\x")),
            Some(r"software\flaree2e\x\Classes".to_string())
        );
    }

    #[test]
    fn an_e2e_run_without_a_sandbox_root_writes_no_verb() {
        let dir = tempfile::tempdir().unwrap();
        for root in [
            None,
            Some(r"Software\Classes"),
            Some(r"Software\FlareE2E\..\x"),
        ] {
            let s = ship(true, true, false, false, root);
            assert!(!s.allowed());
            let fake = Arc::new(FakeRegistry::new());
            let sw = s.verb_switch(fake.clone(), dir.path());
            sw.reconcile();
            assert_eq!(sw.writes(), 0);
            assert_eq!(fake.writes(), 0);
        }
    }

    #[test]
    fn a_dev_build_never_writes_the_verb() {
        let dir = tempfile::tempdir().unwrap();
        let s = ship(false, false, false, false, None);
        assert!(!s.allowed());
        let fake = Arc::new(FakeRegistry::new());
        let sw = s.verb_switch(fake.clone(), dir.path());
        sw.reconcile();
        assert!(!sw.set(true));
        assert!(!sw.set(false));
        assert_eq!(sw.writes(), 0);
        assert_eq!(fake.writes(), 0);
    }

    #[test]
    fn an_e2e_build_writes_only_under_the_sandbox_root() {
        let dir = tempfile::tempdir().unwrap();
        let s = ship(false, true, false, false, Some(r"Software\FlareE2E\t1"));
        assert!(s.sandboxed && s.allowed());
        assert_eq!(s.root, r"Software\FlareE2E\t1\Classes");
        let fake = Arc::new(FakeRegistry::new());
        let sw = s.verb_switch(fake.clone(), dir.path());
        sw.reconcile();
        assert!(sw.writes() > 0);
        let keys = fake.keys();
        assert!(!keys.is_empty());
        for k in &keys {
            let k = k.to_lowercase();
            let sandbox = r"software\flaree2e\t1\classes";
            // Under the sandbox, or one of its own parent keys; never the real classes hive.
            assert!(k.starts_with(sandbox) || sandbox.starts_with(&k), "{k}");
            assert!(!k.starts_with(r"software\classes"), "{k}");
        }
    }

    #[test]
    fn the_mode_follows_d11() {
        for bits in 0u8..16 {
            let (p, e, f, l) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let s = ship(p, e, f, l, None);
            assert_eq!(s.mode, mode_for(p, e, f, l), "{bits:04b}");
        }
    }

    #[test]
    fn the_verb_exe_is_the_current_folder_exe_when_packaged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Flare");
        let cur = root.join("current");
        std::fs::create_dir_all(&cur).unwrap();
        std::fs::write(root.join("Update.exe"), b"x").unwrap();
        let exe = cur.join("Flare.exe");
        std::fs::write(&exe, b"x").unwrap();
        let s = Ship::from_parts(
            exe.clone(),
            packaged::is_packaged(&exe),
            false,
            false,
            false,
            None,
            None,
        );
        assert!(s.packaged);
        assert_eq!(s.verb_exe, Some(exe.to_string_lossy().into_owned()));
        assert!(s.allowed());
        let dev = Ship::from_parts(
            dir.path().join("Flare.exe"),
            false,
            false,
            false,
            false,
            None,
            None,
        );
        assert_eq!(dev.verb_exe, None);
    }

    #[test]
    fn the_settings_row_uses_the_registry_backend() {
        // The production Deps.verb is a RegistryVerb over the switch (window.rs); a type check.
        let dir = tempfile::tempdir().unwrap();
        let s = ship(false, false, false, false, None);
        let sw = s.verb_switch(Arc::new(FakeRegistry::new()), dir.path());
        let _verb: Arc<dyn flare_settings::verb::ExplorerVerb> =
            Arc::new(flare_win::verb_switch::RegistryVerb::new(sw));
        assert!(code_of(&src("window.rs")).contains("RegistryVerb::new"));
        assert!(!code_of(&src("window.rs")).contains("PendingVerb"));
    }

    #[test]
    fn the_feed_follows_the_seams() {
        let none = |_: &str| None;
        // Dev: preview, no feed to speak of.
        let c = choose_feed(&ship(false, false, false, false, None), &none);
        assert_eq!(c.mode, Mode::Preview);
        assert!(c.preset_offer.is_none());
        // An e2e offer: the mode stays Off and the offer is preset.
        let env: HashMap<&str, &str> =
            [("UPDATE_OFFER", "1"), ("UPDATE_NOTES", "## Fixed\n* a")].into();
        let get = move |k: &str| env.get(k).map(|v| (*v).to_string());
        let c = choose_feed(&ship(true, true, false, false, None), &get);
        assert_eq!(c.mode, Mode::Off);
        let o = c.preset_offer.unwrap();
        assert_eq!(
            (o.version.as_str(), o.notes.as_str()),
            ("99.0.0", "## Fixed\n* a")
        );
        assert!(!o.mock);
        // The offer variable outside e2e is ignored.
        let c = choose_feed(&ship(true, false, false, false, None), &get);
        assert!(c.preset_offer.is_none());
        assert_eq!(c.mode, Mode::Off);
    }

    #[test]
    fn the_window_icon_comes_from_resource_one() {
        assert!(code_of(&src("ship_ctl.rs")).contains("ICON_RESOURCE_ID: usize = 1;"));
        assert!(code_of(&src("rc_text.rs")).contains("1 ICON"));
        let startup = code_of(&src("startup.rs")).to_string();
        assert!(startup.contains("ship_ctl::set_window_icon"));
        let window = code_of(&src("window.rs")).to_string();
        // After the window exists, and in the scale-factor hook.
        assert!(window.matches("startup::apply_window_icon").count() >= 2);
        let hook = window
            .split("WindowEvent::ScaleFactorChanged")
            .nth(1)
            .unwrap();
        assert!(
            hook.split("WindowEvent::KeyboardInput")
                .next()
                .unwrap()
                .contains("startup::apply_window_icon")
        );
    }

    #[test]
    fn an_argv_with_the_preview_flag_opens_no_tab() {
        let dir = tempfile::tempdir().unwrap();
        let argv = vec!["flare.exe".to_string(), "--preview-update".to_string()];
        assert!(flare_win::argv::folders_from_argv(&argv, &[], dir.path()).is_empty());
    }

    #[test]
    fn set_window_icon_ignores_a_null_window() {
        set_window_icon(0);
    }

    #[test]
    fn aumid_is_set_in_every_build() {
        assert_eq!(AUMID, "com.flare.terminal");
        let ship = code_of(&src("ship_ctl.rs")).to_string();
        let body = ship.split("pub fn bootstrap()").nth(1).unwrap();
        assert!(body.split("\n}\n").next().unwrap().contains("set_aumid();"));
        assert!(src("main.rs").contains("ship_ctl::bootstrap();"));
    }

    #[test]
    fn the_flare_e2e_names_are_read_only_here() {
        // Every FLARE_E2E_ read in this module goes through `e2e_var`.
        let code = code_of(&src("ship_ctl.rs")).to_string();
        assert!(!code.contains(&["\"FLARE_E2E", "_"].concat()));
    }
}
