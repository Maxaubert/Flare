//! profile, flags, single instance, asks, end of life (Task 29).
//!
//! Source: PT/src/main/index.ts (profile, second-instance, pendingFolders, raise), closeFlow.ts and
//! App.tsx (the close questions). Everything here is plain data and decisions; AppCtl (Task 30) acts.

#![allow(dead_code)] // wired by AppCtl (a later task); remove this line then

use crate::tab_host::TabHost;
use flare_agent::close::{
    CloseTarget, close_primary_label, close_question_title, holds_window_close,
};
use flare_agent::kinds::DetectedAgent;
use flare_tabs::close_flow::{NAME_INK, close_question_body};
use flare_tabs::tabs_store::TabsStore;
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

pub fn ask_view(a: &Ask) -> AskView {
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
        body: close_question_body(a.agent, &a.label, a.for_ms, a.others, a.install, NAME_INK),
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

#[cfg(test)]
mod tests {
    use super::*;
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
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test
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
            std::thread::sleep(Duration::from_millis(300)); // sync-ok: test
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
        let idle = ask_view(&tab_ask("t1", "api", DetectedAgent::Claude, None));
        assert_eq!(idle.title, "Close the tab and end the agent?");
        assert!(idle.body.starts_with(&format!(
            "{} is running in {}",
            names_of("Claude", NAME_INK),
            names_of("api", NAME_INK)
        )));
        assert_eq!(idle.primary, "Close tab");
        let busy = ask_view(&tab_ask("t1", "api", DetectedAgent::Claude, Some(125_000)));
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
        let v = ask_view(&ask);
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
        let v = ask_view(&ask);
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
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test
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
