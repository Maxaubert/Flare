//! profile, flags, single instance, asks, end of life (Task 29).
//!
//! Source: PT/src/main/index.ts (profile, second-instance, pendingFolders, raise), closeFlow.ts and
//! App.tsx (the close questions). Everything here is plain data and decisions; AppCtl (Task 30) acts.

#![allow(dead_code)] // wired by AppCtl (a later task); remove this line then

use crate::config::{Args, RendererChoice};
use crate::tab_host::TabHost;
use flare_agent::close::{
    CloseTarget, close_primary_label, close_question_title, holds_window_close,
};
use flare_agent::kinds::DetectedAgent;
use flare_diag::control::DiagControl;
use flare_settings::import::run::{ImportPaths, ImportReport, run_once, should_run};
use flare_settings::kv::PrefStore;
use flare_settings::page::GlassSupport;
use flare_settings::verb::DiagControlApi;
use flare_tabs::close_flow::close_question_body;
use flare_tabs::tabs_store::TabsStore;
use flare_win::material::{Support, support};
use flare_win::os_build::OsVersion;
use flare_win::single_instance::{
    Acquired, Handoff, InstanceGuard, InstanceNames, acquire, encode_handoff, instance_names,
    send_handoff, serve,
};
use flare_win::window_state::WindowStateStore;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The profile folder: the flag's path, else `<APPDATA>\Flare`, else `<temp>\Flare`.
pub fn profile_dir(user_data_dir: Option<&Path>, appdata: Option<&str>) -> PathBuf {
    if let Some(dir) = user_data_dir {
        return dir.to_path_buf();
    }
    match appdata {
        Some(a) if !a.is_empty() => PathBuf::from(a).join("Flare"),
        _ => std::env::temp_dir().join("Flare"),
    }
}

/// The three state files of a profile.
pub struct Profile {
    pub dir: PathBuf,
}

impl Profile {
    pub fn tabs_file(&self) -> PathBuf {
        self.dir.join("tabs.json")
    }

    pub fn window_file(&self) -> PathBuf {
        self.dir.join("window.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// The diagnostics log folder (M9 Task 31): `<profile>\logs`, whichever way the profile was chosen.
    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
}

/// Starts the diagnostics (M9 Task 31): the log, the session line, the panic hooks, the watchdog and the
/// memory watch. Called once the profile folder is known and this run is the first instance, before the
/// window and the tabs. `backend` is the renderer's name.
pub fn start_diagnostics(profile: &Profile, backend: &str, e2e: bool) {
    // sync-ok: startup, before any window; a registry read and a memory query, microseconds
    let facts = crate::diag_ctl::SystemFacts::gather();
    let ctl = crate::diag_ctl::start(&profile.dir, facts, backend, e2e);
    crate::diag_ctl::install(ctl);
    crate::diag_ctl::install_kill_hook();
}

/// How this process starts: as the first instance, or its folders were handed to a running one.
pub enum Launch {
    /// `guard` is None when the lock itself could not be made (the app runs anyway).
    First {
        guard: Option<InstanceGuard>,
        server_names: InstanceNames,
    },
    HandedOver,
}

/// Acquire the lock. Second: hand `{ cwd, argv }` over (waiting up to `wait`); done, the caller exits 0.
/// A failed hand-off retries the lock ONCE (the first instance may have been quitting): First runs as
/// the first instance, still Second is an error (exit 1).
pub fn launch(profile: &Profile, argv: &[String], wait: Duration) -> Result<Launch, String> {
    let names = instance_names(&profile.dir);
    match acquire(&names) {
        Err(e) => {
            log::warn!("running without the single-instance lock: {e}");
            Ok(Launch::First {
                guard: None,
                server_names: names,
            })
        }
        Ok(Acquired::First(guard)) => Ok(Launch::First {
            guard: Some(guard),
            server_names: names,
        }),
        Ok(Acquired::Second) => {
            let cwd = std::env::current_dir() // sync-ok: startup, before any window
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let handoff = Handoff {
                cwd,
                args: argv.to_vec(),
            };
            match send_handoff(&names, &handoff, wait) {
                Ok(()) => Ok(Launch::HandedOver),
                Err(flare_win::single_instance::HandoffError::Untrusted(e)) => {
                    // Something answered the name that is not this user in this session: write
                    // nothing to it and run as a normal instance (no lock to hold).
                    log::warn!(
                        "the hand-off server was not trusted ({e}); running as a normal instance"
                    );
                    Ok(Launch::First {
                        guard: None,
                        server_names: names,
                    })
                }
                Err(e) => match acquire(&names) {
                    Ok(Acquired::First(guard)) => {
                        log::warn!("the running Flare did not answer ({e}); running as the first");
                        Ok(Launch::First {
                            guard: Some(guard),
                            server_names: names,
                        })
                    }
                    Ok(Acquired::Second) => Err(format!(
                        "another Flare holds the lock but did not answer: {e}"
                    )),
                    Err(e2) => Err(e2),
                },
            }
        }
    }
}

/// Folders handed over before the first restore answered are queued and sent after the reply;
/// after it they open at once. Each is always a new tab.
pub struct PendingFolders {
    awaiting: bool,
    queue: Vec<String>,
}

impl Default for PendingFolders {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingFolders {
    pub fn new() -> Self {
        Self {
            awaiting: true,
            queue: Vec::new(),
        }
    }

    /// What to open NOW.
    pub fn push(&mut self, folders: Vec<String>) -> Vec<String> {
        if self.awaiting {
            self.queue.extend(folders);
            Vec::new()
        } else {
            folders
        }
    }

    /// The restore answered: the queue, once.
    pub fn restored(&mut self) -> Vec<String> {
        self.awaiting = false;
        std::mem::take(&mut self.queue)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaiseStep {
    ShowInactive,
    Restore,
    Show,
    TopmostOn,
    Focus,
    TopmostOff,
}

/// Bringing the window to the front. Under e2e it only shows, inactive.
pub fn raise_plan(e2e: bool, minimised: bool) -> Vec<RaiseStep> {
    if e2e {
        return vec![RaiseStep::ShowInactive];
    }
    let mut steps = Vec::new();
    if minimised {
        steps.push(RaiseStep::Restore);
    }
    steps.extend([
        RaiseStep::Show,
        RaiseStep::TopmostOn,
        RaiseStep::Focus,
        RaiseStep::TopmostOff,
    ]);
    steps
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskTarget {
    Tab(String),
    Window,
}

/// The close question as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub target: AskTarget,
    pub label: String,
    pub agent: DetectedAgent,
    pub for_ms: Option<u64>,
    pub others: usize,
    pub install: bool,
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskView {
    pub title: &'static str,
    pub body: String,
    pub primary: &'static str,
}

/// Closing one tab that hosts an agent; `working_for_ms` None when the agent is only present.
pub fn tab_ask(
    tab_id: &str,
    label: &str,
    agent: DetectedAgent,
    working_for_ms: Option<u64>,
) -> Ask {
    Ask {
        target: AskTarget::Tab(tab_id.to_string()),
        label: label.to_string(),
        agent,
        for_ms: working_for_ms,
        others: 0,
        install: false,
    }
}

/// Closing the window: the agent working LONGEST is named, the others counted. None when nothing holds it.
pub fn window_ask(busy: &[(String, DetectedAgent, u64)]) -> Option<Ask> {
    if !holds_window_close(busy.len()) {
        return None;
    }
    let (label, agent, for_ms) = busy.iter().max_by_key(|(_, _, ms)| *ms)?;
    Some(Ask {
        target: AskTarget::Window,
        label: label.clone(),
        agent: *agent,
        for_ms: Some(*for_ms),
        others: busy.len() - 1,
        install: false,
    })
}

/// The close question. `ink` is the theme's text colour (the names in the body wear it; tests pass `NAME_INK`).
pub fn ask_view(a: &Ask, ink: &str) -> AskView {
    let target = if a.install {
        CloseTarget::Install
    } else {
        match a.target {
            AskTarget::Tab(_) => CloseTarget::Tab,
            AskTarget::Window => CloseTarget::Window,
        }
    };
    AskView {
        title: close_question_title(target, a.for_ms),
        body: close_question_body(a.agent, &a.label, a.for_ms, a.others, a.install, ink),
        primary: close_primary_label(target),
    }
}

/// Something that can be shut down and waited for (the tab host; a fake in tests).
pub trait ShutsDown {
    fn shutdown(&mut self) -> bool;
}

impl ShutsDown for TabHost {
    fn shutdown(&mut self) -> bool {
        TabHost::shutdown(self)
    }
}

/// The end of life, in PT's close-handler order: flush both files, then kill and wait.
pub fn end_of_life(tabs: &TabsStore, window: &WindowStateStore, host: &mut dyn ShutsDown) -> bool {
    window.flush();
    tabs.flush();
    host.shutdown()
}

/// `--instance-probe` (Task 34): the real startup (profile, `launch`, `serve`) with NO window.
/// Prints `names <mutex> <pipe>`, then `first`, then one `handoff <json>` line per hand-off, and
/// exits 0 when stdin closes. A process that is not the first prints `second` and returns 1.
pub fn run_instance_probe(user_data_dir: Option<&Path>, argv: &[String]) -> i32 {
    use std::io::{Read, Write};
    let Some(dir) = user_data_dir else {
        eprintln!("flare: --instance-probe needs --user-data-dir=<path>");
        return 1;
    };
    let profile = Profile {
        dir: dir.to_path_buf(),
    };
    if let Err(e) = std::fs::create_dir_all(&profile.dir) {
        // sync-ok: a hidden test mode, before anything else
        eprintln!("flare: cannot create the profile folder: {e}");
        return 1;
    }
    let launched = match launch(&profile, argv, Duration::from_secs(3)) {
        Ok(l) => l,
        Err(message) => {
            eprintln!("flare: {message}");
            return 1;
        }
    };
    let (guard, names) = match launched {
        Launch::First {
            guard,
            server_names,
        } => (guard, server_names),
        Launch::HandedOver => {
            println!("second");
            return 1;
        }
    };
    let server = match serve(&names, |h| {
        let json = String::from_utf8_lossy(&encode_handoff(&h)).into_owned();
        println!("handoff {json}");
        let _ = std::io::stdout().flush();
    }) {
        Ok(s) => s,
        Err(message) => {
            eprintln!("flare: {message}");
            return 1;
        }
    };
    // The worker must reach its connect call before a second launch writes (a client that writes
    // and closes before then is dropped), so `first` is printed after a short settle.
    std::thread::sleep(Duration::from_millis(200)); // sync-ok: a hidden test mode
    println!("names {} {}", names.mutex, names.pipe);
    println!("first");
    let _ = std::io::stdout().flush();
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink); // sync-ok: the probe lives until stdin closes
    server.stop();
    drop(guard);
    0
}

/// How long `--live-tabs` waits for every shell to show a prompt.
const PROMPT_WAIT: Duration = Duration::from_secs(30);

/// Whether a shell tab has drawn anything: its grid holds a non-blank cell.
fn shows_prompt(h: &flare_term::handle::TermHandle) -> bool {
    let g = h.snapshot();
    (0..g.rows()).any(|row| (0..g.cols()).any(|col| !g.cell(col, row).ch.is_whitespace()))
}

/// `--live-tabs <n>` (Task 34): restore the profile's `tabs.json`, open `n` more real shell tabs,
/// wait (30 s) until each shows a prompt, save, then take the close path (`end_of_life`: flush both
/// files, kill every shell, wait 3 s). Prints `spawn <cwd>` per shell, then `quit-ok <ms>`. No window.
pub fn run_live_tabs(user_data_dir: Option<&Path>, n: u32) -> i32 {
    use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, resolve_restore};
    use flare_term::pty::PortableSpawner;
    use flare_term::session::RealShells;
    use std::io::Write;
    use std::sync::Arc;

    let Some(dir) = user_data_dir else {
        eprintln!("flare: --live-tabs needs --user-data-dir=<path>");
        return 1;
    };
    let profile = Profile {
        dir: dir.to_path_buf(),
    };
    let _ = std::fs::create_dir_all(&profile.dir); // sync-ok: a hidden test mode
    let tabs = TabsStore::new(profile.tabs_file(), Duration::from_millis(400));
    let window = WindowStateStore::new(profile.window_file(), Duration::from_millis(400));
    let prefs = flare_settings::kv::PrefStore::memory();
    let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let mut host = TabHost::new(
        Arc::new(PortableSpawner),
        Arc::new(RealShells),
        prefs,
        waker,
        None,
    );

    let saved = tabs.load();
    // sync-ok: a hidden test mode, no UI thread
    let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|p| Path::new(p).is_dir());
    let plan = resolve_restore(&saved, is_dir, Arc::new(NoSessions));
    let mut now: Vec<SavedTab> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut open = |host: &mut TabHost, cwd: String| {
        let id = format!("t{}", ids.len());
        println!("spawn {cwd}");
        host.spawn(&id, &cwd, None);
        ids.push(id);
        now.push(SavedTab { cwd, agent: None });
    };
    for t in &plan.tabs {
        open(&mut host, t.cwd.clone());
    }
    let active = plan.active;
    let fresh = std::env::temp_dir().to_string_lossy().into_owned();
    for _ in 0..n {
        open(&mut host, fresh.clone());
    }
    let _ = std::io::stdout().flush();

    let deadline = std::time::Instant::now() + PROMPT_WAIT;
    let mut ready = vec![false; ids.len()];
    while ready.iter().any(|r| !r) {
        if std::time::Instant::now() > deadline {
            eprintln!("flare: a shell showed no prompt within 30 s");
            host.shutdown();
            return 1;
        }
        for (i, id) in ids.iter().enumerate() {
            if !ready[i]
                && let Some(h) = host.handle(id)
            {
                let _ = h.drain_events();
                ready[i] = shows_prompt(&h);
            }
        }
        std::thread::sleep(Duration::from_millis(50)); // sync-ok: a hidden test mode
    }

    let count = now.len();
    tabs.save(&SavedTabs {
        tabs: now,
        active: active.min(count.saturating_sub(1)),
    });
    let started = std::time::Instant::now();
    let ok = end_of_life(&tabs, &window, &mut host);
    let ms = started.elapsed().as_millis();
    if ok {
        println!("quit-ok {ms}");
        let _ = std::io::stdout().flush();
        0
    } else {
        eprintln!("flare: the quit gave up waiting for the shells");
        1
    }
}

/// The folder of the Claude plugin, found ONCE at startup (two small stats and the install of the
/// plugin copy). None under `--e2e`: a test run never installs into the real profile folder.
pub fn production_plugin_dir(e2e: bool) -> Option<String> {
    if e2e {
        return None;
    }
    // sync-ok: startup, two small stats
    flare_agent::plugin::plugin_dir().map(|p| p.to_string_lossy().into_owned())
}

/// Puts the exe's own icon on the window (M9 Task 32): once its handle exists, and again on every
/// scale-factor change (the icon sizes follow the monitor's DPI).
pub fn apply_window_icon(hwnd: Option<isize>) {
    if let Some(hwnd) = hwnd {
        crate::ship_ctl::set_window_icon(hwnd);
    }
}

/// The restore's Claude session lookup: the real transcript store of the home folder.
pub fn production_lookup() -> std::sync::Arc<dyn flare_tabs::restore::SessionLookup> {
    std::sync::Arc::new(crate::sessions::ClaudeSessionLookup::from_env())
}

// ---- M7 Task 44: the first-run import, the Settings ports and the warm-up ----------------------

/// The licence file; its `Copyright` line is the About page's copyright (decision: read, never typed).
pub const LICENCE: &str = include_str!("../../../LICENSE");

/// The whole import's ceiling, shared by every Prism Terminal profile: one bounded wait before the first paint
/// (decision D14, M10 Task 53).
pub const IMPORT_CEILING: Duration = Duration::from_millis(1500);

/// Where the first-run import reads from, or why it does not run this start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportPlan {
    Skip,
    Run { appdata: PathBuf },
}

/// The import's gate. Headless modes and `--no-import` never import. Under `--e2e` only the seam's
/// folder is read (never the owner's Prism Terminal data), and no seam means no import. Otherwise
/// the folder is `%APPDATA%`.
pub fn import_plan(args: &Args, e2e_seam: Option<&str>, appdata: Option<&str>) -> ImportPlan {
    let headless = args.sample
        || args.scene.is_some()
        || args.screenshot.is_some()
        || args.live_screenshot.is_some()
        || args.instance_probe
        || args.probe_backend
        || args.bench_frames.is_some();
    if args.no_import || headless {
        return ImportPlan::Skip;
    }
    let folder = if args.e2e { e2e_seam } else { appdata };
    match folder.filter(|f| !f.is_empty()) {
        Some(f) => ImportPlan::Run {
            appdata: PathBuf::from(f),
        },
        None => ImportPlan::Skip,
    }
}

/// Runs the once-only import when the plan says so and the settings file does not record it yet.
/// Called before anything reads a setting; the whole import waits at most `IMPORT_CEILING`. None when nothing ran.
pub fn import_at_start(
    prefs: &PrefStore,
    plan: &ImportPlan,
    temp_root: &Path,
) -> Option<ImportReport> {
    let ImportPlan::Run { appdata } = plan else {
        return None;
    };
    if !should_run(prefs) {
        return None;
    }
    let paths = ImportPaths {
        appdata: appdata.clone(),
        temp_root: temp_root.to_path_buf(),
    };
    // sync-ok: once, before the first paint, bounded by the ceiling
    Some(run_once(prefs, &paths, IMPORT_CEILING))
}

/// The diagnostics control as the Settings page sees it: the real flag file and folder. The mark
/// goes to no sink until M9 owns the diagnostics writer.
pub struct DiagPort(pub DiagControl);

impl DiagControlApi for DiagPort {
    fn verbose(&self) -> bool {
        self.0.verbose()
    }

    fn set_verbose(&self, on: bool) -> bool {
        self.0.set_verbose(on)
    }

    fn mark(&self) {
        self.0.mark();
    }

    fn dir(&self) -> PathBuf {
        self.0.dir()
    }
}

/// The first line of the licence that starts with `Copyright`, trimmed; empty when there is none.
pub fn copyright_line(licence: &str) -> String {
    licence
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Copyright"))
        .unwrap_or_default()
        .to_string()
}

/// What the glass rows may say: See-through needs the GPU renderer and Windows 11 22H2 (the DWM
/// backdrop build); the older builds and the software renderer say why.
pub fn glass_support(effective: RendererChoice, os: OsVersion) -> GlassSupport {
    if effective != RendererChoice::Gpu {
        return GlassSupport::NeedsGraphicsRenderer;
    }
    match support(os) {
        Support::Ok => GlassSupport::Ok,
        Support::NeedsWindows11 => GlassSupport::NeedsWindows11,
        Support::NeedsWindows11_22H2 => GlassSupport::NeedsWindows11Update,
    }
}

/// Builds the help catalogue and runs one search on a worker, so the first query is quick.
pub fn warm_help_index() {
    let spawned = std::thread::Builder::new()
        .name("flare-help-warm".into())
        .spawn(|| {
            let all = flare_help::entries::all_help();
            let _ = flare_help::search::search_help(all, "warm", &Default::default());
        });
    if let Err(e) = spawned {
        log::warn!("could not warm the help index: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_tabs::close_flow::NAME_INK;
    use flare_tabs::model::TabModel;
    use flare_tabs::restore::{Restored, RestoredTab, SavedTab, SavedTabs};
    use flare_win::window_state::WindowState;
    use std::sync::mpsc;
    use std::time::Instant;

    fn names_of(a: &str, ink: &str) -> String {
        format!("<font color=\"{ink}\">{a}</font>")
    }

    #[test]
    fn the_profile_is_the_flag_else_appdata_flare() {
        assert_eq!(
            profile_dir(Some(Path::new("C:\\p")), Some("C:\\A")),
            PathBuf::from("C:\\p")
        );
        assert_eq!(
            profile_dir(None, Some("C:\\Users\\x\\AppData\\Roaming")),
            PathBuf::from("C:\\Users\\x\\AppData\\Roaming").join("Flare")
        );
        assert!(profile_dir(None, None).ends_with("Flare"));
    }

    #[test]
    fn the_three_state_files() {
        let p = Profile {
            dir: PathBuf::from("C:\\p"),
        };
        assert_eq!(p.tabs_file(), PathBuf::from("C:\\p").join("tabs.json"));
        assert_eq!(p.window_file(), PathBuf::from("C:\\p").join("window.json"));
        assert_eq!(
            p.settings_file(),
            PathBuf::from("C:\\p").join("settings.json")
        );
    }

    fn temp_profile() -> (tempfile::TempDir, Profile) {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            dir: dir.path().to_path_buf(),
        };
        (dir, profile)
    }

    #[test]
    fn a_second_launch_hands_over_and_a_first_runs() {
        let (_d, profile) = temp_profile();
        let argv = vec!["C:\\work".to_string()];
        let first = launch(&profile, &argv, Duration::from_millis(200)).unwrap();
        let Launch::First {
            guard: _guard,
            server_names,
        } = first
        else {
            panic!("the first launch must be First");
        };
        // No server yet: the second launch cannot hand over and cannot win the lock.
        assert!(launch(&profile, &argv, Duration::from_millis(150)).is_err());
        let (tx, rx) = mpsc::channel();
        let server = flare_win::single_instance::serve(&server_names, move |h| {
            let _ = tx.send(h);
        })
        .unwrap();
        // Let the worker reach its connect call: a client that writes and closes before then is
        // dropped by single_instance (reported to the orchestrator).
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test code only, never the app
        let second = launch(&profile, &argv, Duration::from_secs(3)).unwrap();
        assert!(matches!(second, Launch::HandedOver));
        let got = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(got.args, argv);
        assert!(!got.cwd.is_empty());
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test, the worker must be listening again
        server.stop();
    }

    #[test]
    fn another_profile_is_its_own_first_instance() {
        let (_a, pa) = temp_profile();
        let (_b, pb) = temp_profile();
        let one = launch(&pa, &[], Duration::from_millis(100)).unwrap();
        let two = launch(&pb, &[], Duration::from_millis(100)).unwrap();
        assert!(matches!(one, Launch::First { .. }));
        assert!(matches!(two, Launch::First { .. }));
    }

    #[test]
    fn a_handoff_to_a_quitting_instance_runs_as_first() {
        let (_d, profile) = temp_profile();
        let names = instance_names(&profile.dir);
        let Ok(Acquired::First(guard)) = acquire(&names) else {
            panic!("the test should win the lock");
        };
        let dropper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300)); // sync-ok: test code only, never the app
            drop(guard);
        });
        let started = Instant::now();
        let got = launch(&profile, &[], Duration::from_millis(600)).unwrap();
        assert!(matches!(got, Launch::First { guard: Some(_), .. }));
        assert!(started.elapsed() >= Duration::from_millis(300));
        dropper.join().unwrap();

        // With the lock kept for the whole call it is an error.
        let (_d2, profile2) = temp_profile();
        let names2 = instance_names(&profile2.dir);
        let Ok(Acquired::First(_held)) = acquire(&names2) else {
            panic!("the test should win the lock");
        };
        assert!(launch(&profile2, &[], Duration::from_millis(200)).is_err());
    }

    #[test]
    fn folders_before_the_restore_are_queued_and_sent_after_the_reply() {
        let mut p = PendingFolders::new();
        assert!(p.push(vec!["C:\\a".into()]).is_empty());
        assert_eq!(p.restored(), vec!["C:\\a".to_string()]);
        assert!(p.restored().is_empty());
        assert_eq!(p.push(vec!["C:\\b".into()]), vec!["C:\\b".to_string()]);
    }

    #[test]
    fn raise_is_restore_show_topmost_focus_untopmost_and_inactive_under_e2e() {
        use RaiseStep::*;
        assert_eq!(
            raise_plan(false, true),
            vec![Restore, Show, TopmostOn, Focus, TopmostOff]
        );
        assert!(!raise_plan(false, false).contains(&Restore));
        assert_eq!(raise_plan(true, true), vec![ShowInactive]);
        assert_eq!(raise_plan(true, false), vec![ShowInactive]);
    }

    #[test]
    fn a_tab_ask_names_the_agent_and_says_working_or_idle() {
        let idle = ask_view(&tab_ask("t1", "api", DetectedAgent::Claude, None), NAME_INK);
        assert_eq!(idle.title, "Close the tab and end the agent?");
        assert!(idle.body.starts_with(&format!(
            "{} is running in {}",
            names_of("Claude", NAME_INK),
            names_of("api", NAME_INK)
        )));
        assert_eq!(idle.primary, "Close tab");
        let busy = ask_view(
            &tab_ask("t1", "api", DetectedAgent::Claude, Some(125_000)),
            NAME_INK,
        );
        assert_eq!(busy.title, "Stop the agent and close?");
        assert!(busy.body.contains("working for 2 minutes"));
    }

    #[test]
    fn the_window_ask_names_the_longest_and_counts_the_others() {
        let busy = vec![
            ("a".to_string(), DetectedAgent::Claude, 60_000),
            ("b".to_string(), DetectedAgent::Codex, 300_000),
            ("c".to_string(), DetectedAgent::Claude, 10_000),
        ];
        let ask = window_ask(&busy).unwrap();
        assert_eq!(ask.label, "b");
        assert_eq!(ask.agent, DetectedAgent::Codex);
        assert_eq!(ask.for_ms, Some(300_000));
        assert_eq!(ask.others, 2);
        let v = ask_view(&ask, NAME_INK);
        assert_eq!(v.title, "Stop the agent and close the window?");
        assert_eq!(v.primary, "Close window");
        let want = format!(
            "working for 5 minutes in {}, and 2 other tabs are working too. ",
            names_of("b", NAME_INK)
        );
        assert!(v.body.contains(&want), "{}", v.body);
    }

    #[test]
    fn nobody_working_means_no_window_ask() {
        assert_eq!(window_ask(&[]), None);
    }

    #[test]
    fn an_install_ask_says_install() {
        let mut ask = tab_ask("t1", "api", DetectedAgent::Claude, Some(90_000));
        ask.install = true;
        let v = ask_view(&ask, NAME_INK);
        assert_eq!(v.title, "Stop the agent and install the update?");
        assert_eq!(v.primary, "Install and restart");
    }

    struct FakeHost {
        tabs_file: PathBuf,
        window_file: PathBuf,
        saw: Option<(bool, bool)>,
    }

    impl ShutsDown for FakeHost {
        fn shutdown(&mut self) -> bool {
            self.saw = Some((self.tabs_file.exists(), self.window_file.exists()));
            true
        }
    }

    #[test]
    fn end_of_life_flushes_both_files_then_kills_and_waits() {
        let (_d, profile) = temp_profile();
        let tabs = TabsStore::new(profile.tabs_file(), Duration::from_secs(60));
        let window = WindowStateStore::new(profile.window_file(), Duration::from_secs(60));
        tabs.save(&SavedTabs {
            tabs: vec![SavedTab {
                cwd: "C:\\a".into(),
                agent: None,
            }],
            active: 0,
        });
        window.save(&WindowState {
            width: 1180,
            height: 780,
            x: Some(10),
            y: Some(10),
            maximised: false,
            scale: 1.0,
        });
        assert!(!profile.tabs_file().exists() && !profile.window_file().exists());
        let mut host = FakeHost {
            tabs_file: profile.tabs_file(),
            window_file: profile.window_file(),
            saw: None,
        };
        assert!(end_of_life(&tabs, &window, &mut host));
        assert_eq!(host.saw, Some((true, true)));
    }

    #[test]
    fn nothing_is_written_before_the_restore_answered() {
        let (_d, profile) = temp_profile();
        let tabs = TabsStore::new(profile.tabs_file(), Duration::from_millis(20));
        let saved = SavedTabs {
            tabs: vec![SavedTab {
                cwd: "C:\\a".into(),
                agent: None,
            }],
            active: 0,
        };
        let mut model = TabModel::boot(&saved, 1);
        let glue = |m: &TabModel| {
            if let Some(s) = m.saved_if_restored() {
                tabs.save(&s);
            }
        };
        glue(&model);
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test code only, never the app
        tabs.flush();
        assert!(!profile.tabs_file().exists());
        model.settle(&Restored {
            tabs: vec![RestoredTab {
                cwd: "C:\\a".into(),
                resume: None,
                from: Some(0),
            }],
            active: 0,
        });
        glue(&model);
        tabs.flush();
        assert!(profile.tabs_file().exists());
    }
}

#[cfg(test)]
mod m7_start_tests {
    use super::*;
    use flare_settings::kv::{Backend, PrefStore};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn args(words: &[&str]) -> Args {
        let owned: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
        crate::config::parse_args(&owned, None).expect("the flags parse")
    }

    /// Records every get and set, in order, and keeps the values in memory.
    #[derive(Default)]
    struct Recording {
        log: Mutex<Vec<String>>,
        values: Mutex<BTreeMap<String, String>>,
    }

    impl Backend for Recording {
        fn get(&self, key: &str) -> Option<String> {
            self.log.lock().unwrap().push(format!("get {key}"));
            self.values.lock().unwrap().get(key).cloned()
        }

        fn set(&self, key: &str, value: &str) {
            self.log.lock().unwrap().push(format!("set {key}"));
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
        }

        fn remove(&self, key: &str) {
            self.log.lock().unwrap().push(format!("remove {key}"));
            self.values.lock().unwrap().remove(key);
        }
    }

    #[test]
    fn the_import_is_skipped_under_e2e_without_the_seam_and_under_no_import() {
        let appdata = Some("C:\\Users\\someone\\AppData\\Roaming");
        assert_eq!(
            import_plan(&args(&["--e2e"]), None, appdata),
            ImportPlan::Skip
        );
        assert_eq!(
            import_plan(&args(&["--e2e"]), Some(""), appdata),
            ImportPlan::Skip
        );
        assert_eq!(
            import_plan(&args(&["--no-import"]), None, appdata),
            ImportPlan::Skip
        );
        assert_eq!(
            import_plan(&args(&[]), None, None),
            ImportPlan::Skip,
            "no APPDATA, no import"
        );
    }

    #[test]
    fn the_import_is_skipped_in_every_headless_mode() {
        let appdata = Some("C:\\A");
        for words in [
            vec!["--scene", "settings"],
            vec!["--screenshot", "x.png"],
            vec!["--live-screenshot", "x.png"],
            vec!["--instance-probe"],
            vec!["--probe-backend"],
            vec!["--bench-frames", "5"],
            vec!["--sample"],
        ] {
            assert_eq!(
                import_plan(&args(&words), None, appdata),
                ImportPlan::Skip,
                "{words:?}"
            );
        }
        assert_eq!(
            import_plan(&args(&[]), None, appdata),
            ImportPlan::Run {
                appdata: PathBuf::from("C:\\A")
            }
        );
    }

    #[test]
    fn the_e2e_seam_points_the_importer_at_a_test_folder() {
        assert_eq!(
            import_plan(&args(&["--e2e"]), Some("D:\\scratch\\pt"), Some("C:\\A")),
            ImportPlan::Run {
                appdata: PathBuf::from("D:\\scratch\\pt")
            },
            "under e2e the seam wins and %APPDATA% is never read"
        );
    }

    #[test]
    fn a_second_start_does_not_import_again() {
        let appdata = tempfile::tempdir().expect("a temp folder");
        let temp = tempfile::tempdir().expect("a temp folder");
        let prefs = PrefStore::memory();
        let plan = ImportPlan::Run {
            appdata: appdata.path().to_path_buf(),
        };
        let first = import_at_start(&prefs, &plan, temp.path());
        assert!(first.is_some(), "the first start runs the import");
        assert!(prefs.get(flare_settings::import::run::DONE_KEY).is_some());
        assert!(import_at_start(&prefs, &plan, temp.path()).is_none());
    }

    #[test]
    fn no_setting_is_read_before_the_import_records_itself() {
        let appdata = tempfile::tempdir().expect("a temp folder");
        let temp = tempfile::tempdir().expect("a temp folder");
        let backend = Arc::new(Recording::default());
        let prefs = PrefStore::new(backend.clone());
        let plan = ImportPlan::Run {
            appdata: appdata.path().to_path_buf(),
        };
        import_at_start(&prefs, &plan, temp.path());
        let log = backend.log.lock().unwrap().clone();
        let done = format!("set {}", flare_settings::import::run::DONE_KEY);
        let at = log
            .iter()
            .position(|l| *l == done)
            .expect("the import records itself");
        for line in &log[..at] {
            let key = line.trim_start_matches("get ").trim_start_matches("set ");
            assert!(
                key.starts_with("flare.import.") || key.starts_with("flare."),
                "before the record only import keys are touched: {line}"
            );
        }
    }

    #[test]
    fn the_copyright_line_is_read_from_license() {
        let line = copyright_line(LICENCE);
        assert!(line.starts_with("Copyright (c) 2026"), "{line}");
        assert_eq!(copyright_line("no notice here"), "");
    }

    #[test]
    fn glass_says_why_it_is_off() {
        let os = |build| OsVersion {
            major: 10,
            minor: 0,
            build,
        };
        assert_eq!(
            glass_support(RendererChoice::Software, os(26100)),
            GlassSupport::NeedsGraphicsRenderer
        );
        assert_eq!(
            glass_support(RendererChoice::Gpu, os(19045)),
            GlassSupport::NeedsWindows11
        );
        assert_eq!(
            glass_support(RendererChoice::Gpu, os(22000)),
            GlassSupport::NeedsWindows11Update
        );
        assert_eq!(
            glass_support(RendererChoice::Gpu, os(22621)),
            GlassSupport::Ok
        );
    }

    #[test]
    fn the_log_folder_follows_the_profile() {
        let appdata = Some("C:\\Users\\x\\AppData\\Roaming");
        let default = Profile {
            dir: profile_dir(None, appdata),
        };
        assert_eq!(
            default.logs_dir(),
            PathBuf::from("C:\\Users\\x\\AppData\\Roaming\\Flare\\logs")
        );
        let custom = Profile {
            dir: profile_dir(Some(Path::new("D:\\data")), appdata),
        };
        assert_eq!(custom.logs_dir(), PathBuf::from("D:\\data\\logs"));
    }

    #[test]
    fn the_diagnostics_port_reads_and_writes_the_flag_file() {
        let profile = tempfile::tempdir().expect("a temp folder");
        let port = DiagPort(DiagControl::new(profile.path(), None));
        assert!(!port.verbose());
        assert!(port.set_verbose(true));
        assert!(port.verbose());
        assert_eq!(port.dir(), profile.path().join("logs"));
        port.mark();
    }

    #[test]
    fn the_import_runs_before_the_look_and_the_first_paint() {
        let src = include_str!("window.rs");
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("window.rs has {needle}"))
        };
        let import = at("import_at_start(");
        assert!(import < at("TabHost::new("), "the import precedes the tabs");
        assert!(import < at("LookCtl::new("), "the import precedes the look");
        assert!(
            import < at("app.show()?"),
            "the import precedes the first paint"
        );
    }
}
