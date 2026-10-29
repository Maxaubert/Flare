//! Install when idle (2026-10-29): the "Build ready" chip, its menu, the toast, the idle rule and the restart, in the
//! real `AppCtl` over a temp install. Headless: nothing starts a shell, a process or an OS window; the restart is a
//! recorder and the disk work runs as queued jobs.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flare_agent::kinds::AgentKind;
use flare_settings::kv::PrefStore;
use flare_tabs::restore::NoSessions;
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_update::staged::{self, Layout, Manifest, Start};
use flare_update::staged_flow::{Mode, Phase, Times};
use flare_win::window_state::WindowStateStore;
use slint::Model;

use crate::AppWindow;
use crate::appctl::staged_glue::{StagedMsg, StagedParts};
use crate::appctl::{AppCtl, Deps, Post, TermPorts, UiMsg};
use crate::tab_host::TabHost;
use crate::ui_harness::Harness;

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;
type Jobs = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;
type Restarts = Rc<RefCell<Vec<(PathBuf, Vec<String>)>>>;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Queue,
    jobs: Jobs,
    quits: Rc<Cell<u32>>,
    clock: Rc<Cell<u64>>,
    restarts: Restarts,
    layout: Layout,
    _dir: tempfile::TempDir,
}

/// A temp install with an old build in `current\`.
fn install_dir() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("StableFlare"), "FlareStable.exe");
    std::fs::create_dir_all(layout.current()).unwrap();
    std::fs::write(layout.exe(), b"old exe").unwrap();
    std::fs::write(layout.current().join("flare-mcp.exe"), b"old mcp").unwrap();
    (dir, layout)
}

/// Stages a build of `exe` bytes, as `tools/stage-install.ps1` would.
fn stage(layout: &Layout, exe: &[u8]) {
    let s = layout.staged();
    let _ = std::fs::remove_dir_all(&s);
    std::fs::create_dir_all(&s).unwrap();
    let mut files = BTreeMap::new();
    for (name, bytes) in [
        ("FlareStable.exe", exe),
        ("flare-mcp.exe", b"new mcp".as_slice()),
    ] {
        std::fs::write(s.join(name), bytes).unwrap();
        files.insert(
            name.to_string(),
            staged::sha256_file(&s.join(name)).unwrap(),
        );
    }
    let m = Manifest {
        commit: "abc1234def".into(),
        built_at: "2026-10-29T10:00:00Z".into(),
        files,
    };
    std::fs::write(layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
}

fn make(start: Start, staged_first: bool) -> Fx {
    make_with(move |layout| {
        if staged_first {
            stage(layout, b"new exe");
        }
        start
    })
}

/// As `make`, with the temp install prepared by `prep`, which also says what the start found.
fn make_with(prep: impl FnOnce(&Layout) -> Start) -> Fx {
    let (dir, layout) = install_dir();
    let start = prep(&layout);
    let q: Queue = Arc::default();
    let post: Post = {
        let q = q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let prefs = PrefStore::memory();
    let waker: Arc<dyn Fn() + Send + Sync> = {
        let post = post.clone();
        Arc::new(move || post(UiMsg::HostWake))
    };
    let host = TabHost::new(
        FakeSpawner::new(),
        FakeShells::new(),
        prefs.clone(),
        waker,
        None,
    );
    let clock = Rc::new(Cell::new(0u64));
    let quits = Rc::new(Cell::new(0u32));
    let deps = Deps {
        prefs: prefs.clone(),
        tabs_store: Arc::new(TabsStore::new(
            dir.path().join("tabs.json"),
            Duration::from_millis(30),
        )),
        window_store: Arc::new(WindowStateStore::new(
            dir.path().join("window.json"),
            Duration::from_millis(30),
        )),
        host,
        lookup: Arc::new(NoSessions),
        is_dir: Arc::new(|_| true),
        home: "C:\\home".to_string(),
        clock: {
            let c = clock.clone();
            Rc::new(move || c.get())
        },
        post: post.clone(),
        pick_folder: Rc::new(|_, done| done(None)),
        clipboard: Rc::new(|_| {}),
        open_url: Rc::new(|_| {}),
        set_grid_source: Rc::new(|_| {}),
        raise: Rc::new(|| {}),
        quit: {
            let k = quits.clone();
            Rc::new(move || k.set(k.get() + 1))
        },
        hide: Rc::new(|| {}),
        starter_folders: Rc::new(|_| {}),
        launch_folders: Vec::new(),
        e2e: true,
        term: TermPorts::inert(),
    };
    let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
    let ctl = AppCtl::new(&h.comp, deps);
    ctl.borrow_mut().set_animations_source(Rc::new(|| true));
    h.advance(0);
    ctl.borrow_mut().refresh();
    let jobs: Jobs = Rc::default();
    let restarts: Restarts = Rc::default();
    {
        let j = jobs.clone();
        let r = restarts.clone();
        AppCtl::attach_staged(
            &ctl,
            StagedParts {
                layout: layout.clone(),
                start,
                times: Times::default(),
                spawn: Rc::new(move |job| j.borrow_mut().push(job)),
                restart: Rc::new(move |exe: &Path, args: &[String], _dir: &str| {
                    r.borrow_mut().push((exe.to_path_buf(), args.to_vec()));
                    true
                }),
                watch: false,
                user_data_dir: Some(PathBuf::from("C:\\Users\\x\\AppData\\Roaming\\FlareStable")),
                argv: vec!["--user-data-dir=C:\\Users\\x\\AppData\\Roaming\\FlareStable".into()],
                e2e_dir: None,
            },
        );
    }
    let f = Fx {
        h,
        ctl,
        q,
        jobs,
        quits,
        clock,
        restarts,
        layout,
        _dir: dir,
    };
    f.settle();
    f
}

impl Fx {
    fn pump(&self) {
        loop {
            let m = self.q.lock().unwrap().pop_front();
            let Some(m) = m else { break };
            self.ctl.borrow_mut().on_msg(m);
        }
    }

    fn run_jobs(&self) {
        loop {
            let jobs: Vec<_> = self.jobs.borrow_mut().drain(..).collect();
            if jobs.is_empty() {
                break;
            }
            for j in jobs {
                j();
            }
            self.pump();
        }
    }

    /// Runs every queued job and message until nothing is left.
    fn settle(&self) {
        self.run_jobs();
        self.pump();
        self.run_jobs();
    }

    /// The folder changed (what the watch posts).
    fn changed(&self) {
        (self.ctl.borrow().post_for_test())(UiMsg::Staged(StagedMsg::Changed));
        self.pump();
        self.settle();
    }

    fn app(&self) -> &AppWindow {
        &self.h.comp
    }

    /// The clock reads `ms`, and the flow's timer fires.
    fn at(&self, ms: u64) {
        self.clock.set(ms);
        self.h.advance(250);
        self.settle();
    }

    fn phase(&self) -> Phase {
        self.ctl.borrow().staged_flow().unwrap().phase()
    }

    fn agent_working(&self, on: bool) -> String {
        let mut c = self.ctl.borrow_mut();
        let id = c
            .model()
            .state
            .tabs
            .first()
            .map(|t| t.id.clone())
            .unwrap_or_default();
        let id = if id.is_empty() {
            c.open_folder("C:\\a")
        } else {
            id
        };
        c.model_mut().set_agent(&id, Some(AgentKind::Claude));
        c.set_working(&id, on.then_some(0));
        id
    }

    fn menu_labels(&self) -> Vec<(String, bool)> {
        self.app()
            .get_menu_rows()
            .iter()
            .map(|r| (r.label.to_string(), r.disabled))
            .collect()
    }

    fn click_chip(&self) {
        self.app().invoke_staged_chip_clicked(900.0, 6.0, 30.0);
        self.pump();
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.layout.current().join(name)).unwrap_or_default()
    }
}

#[test]
fn no_stage_no_chip_and_a_ready_stage_shows_build_ready() {
    let f = make(Start::Plain, false);
    assert!(!f.app().get_staged_chip_shown());
    assert_eq!(f.app().get_staged_toast(), "");
    stage(&f.layout, b"new exe");
    f.changed();
    let app = f.app();
    assert!(app.get_staged_chip_shown());
    assert_eq!(app.get_staged_chip_label(), "Build ready");
    assert_eq!(
        app.get_staged_chip_aria(),
        "Build ready, installs when idle"
    );
    assert_eq!(
        app.get_staged_chip_tip(),
        "Build abc1234 installs once no agent has worked for a minute"
    );
}

#[test]
fn a_stage_whose_hash_does_not_match_shows_no_chip() {
    let f = make(Start::Plain, false);
    stage(&f.layout, b"new exe");
    std::fs::write(
        f.layout.staged().join("flare-mcp.exe"),
        b"changed after staging",
    )
    .unwrap();
    f.changed();
    assert!(!f.app().get_staged_chip_shown());
}

#[test]
fn the_chip_opens_its_menu_and_install_when_idle_is_the_choice_already_made() {
    let f = make(Start::Plain, true);
    f.click_chip();
    assert!(f.app().get_menu_open());
    assert_eq!(
        f.menu_labels(),
        vec![
            ("Install now".to_string(), false),
            ("Install when idle".to_string(), true),
            ("Not today".to_string(), false),
            ("Discard".to_string(), false),
        ]
    );
}

#[test]
fn a_quiet_minute_counts_down_ten_seconds_then_swaps_and_restarts_on_the_same_profile() {
    let f = make(Start::Plain, true);
    f.at(1_000);
    f.at(60_000);
    assert_eq!(f.phase(), Phase::Waiting, "59 s of quiet is not enough");
    f.at(61_000);
    assert_eq!(
        f.app().get_staged_toast(),
        "Installing the new build and restarting"
    );
    assert_eq!(f.app().get_staged_toast_button(), "Cancel");
    assert!(f.restarts.borrow().is_empty());
    f.at(70_999);
    assert!(f.restarts.borrow().is_empty(), "the full 10 s");
    f.at(71_000);
    assert_eq!(f.read("FlareStable.exe"), b"new exe");
    assert_eq!(f.read("FlareStable.exe.old"), b"old exe");
    let r = f.restarts.borrow();
    assert_eq!(r.len(), 1, "the new exe starts once");
    assert_eq!(r[0].0, f.layout.exe());
    assert_eq!(
        r[0].1,
        vec!["--user-data-dir=C:\\Users\\x\\AppData\\Roaming\\FlareStable".to_string()]
    );
    assert_eq!(f.quits.get(), 1, "through the normal end of life");
    let m = staged::read_marker(&f.layout).unwrap();
    assert_eq!(
        m.state,
        staged::MarkerState::Pending,
        "this run never confirms its own swap"
    );
}

#[test]
fn a_working_agent_or_its_question_holds_the_install_and_starts_the_minute_again() {
    let f = make(Start::Plain, true);
    f.agent_working(true);
    f.at(1_000);
    f.at(200_000);
    assert_eq!(f.phase(), Phase::Waiting, "never while an agent works");
    assert!(f.restarts.borrow().is_empty());
    f.agent_working(false);
    f.at(200_250);
    f.at(260_000);
    assert_eq!(f.phase(), Phase::Waiting);
    f.at(260_250);
    assert!(matches!(f.phase(), Phase::Countdown { by_hand: false, .. }));
    // Work starts during the countdown: it stops, and the minute starts again.
    f.agent_working(true);
    f.at(262_000);
    assert_eq!(f.phase(), Phase::Waiting);
    assert_eq!(f.app().get_staged_toast(), "");
    f.at(400_000);
    assert!(f.restarts.borrow().is_empty());
}

#[test]
fn cancel_on_the_toast_stops_it_and_leaves_the_build_for_a_click() {
    let f = make(Start::Plain, true);
    f.at(1_000);
    f.at(61_000);
    f.app().invoke_staged_toast_clicked();
    f.pump();
    assert_eq!(f.app().get_staged_toast(), "");
    assert_eq!(f.ctl.borrow().staged_flow().unwrap().mode(), Mode::Manual);
    assert_eq!(
        f.app().get_staged_chip_tip(),
        "Build abc1234 is ready to install"
    );
    f.at(500_000);
    assert!(f.restarts.borrow().is_empty());
    assert_eq!(f.read("FlareStable.exe"), b"old exe");
}

#[test]
fn install_now_under_a_working_agent_asks_first_then_counts_down_without_waiting_for_idle() {
    let f = make(Start::Plain, true);
    f.agent_working(true);
    f.at(5_000);
    f.click_chip();
    f.app().invoke_menu_picked(0);
    f.pump();
    let app = f.app();
    assert!(app.get_dialog_open(), "the close question is up");
    assert_eq!(
        app.get_dialog_title().as_str(),
        "Stop the agent and install the update?"
    );
    assert_eq!(app.get_dialog_primary().as_str(), "Install and restart");
    app.invoke_dialog_cancel();
    f.pump();
    assert!(!app.get_dialog_open());
    assert_eq!(f.phase(), Phase::Waiting, "a no installs nothing");

    f.click_chip();
    app.invoke_menu_picked(0);
    f.pump();
    app.invoke_dialog_confirm();
    f.pump();
    assert!(matches!(f.phase(), Phase::Countdown { by_hand: true, .. }));
    f.at(16_000);
    assert_eq!(
        f.restarts.borrow().len(),
        1,
        "a countdown by hand goes on while the agent works"
    );
}

#[test]
fn install_now_with_no_agent_at_work_needs_no_question() {
    let f = make(Start::Plain, true);
    f.click_chip();
    f.app().invoke_menu_picked(0);
    f.pump();
    assert!(!f.app().get_dialog_open());
    assert!(matches!(f.phase(), Phase::Countdown { by_hand: true, .. }));
}

#[test]
fn a_build_that_cannot_be_installed_says_so_and_nothing_changes() {
    let f = make(Start::Plain, true);
    f.click_chip();
    f.app().invoke_menu_picked(0);
    f.pump();
    // The staged exe changes between the check and the swap: the swap's own check refuses it.
    std::fs::write(f.layout.staged().join("FlareStable.exe"), b"tampered").unwrap();
    f.at(10_000);
    assert_eq!(
        f.app().get_staged_toast(),
        "The build could not be installed; nothing changed"
    );
    assert!(f.restarts.borrow().is_empty());
    assert_eq!(f.quits.get(), 0);
    assert_eq!(f.read("FlareStable.exe"), b"old exe");
    assert!(f.app().get_staged_chip_shown(), "the chip stays");
}

#[test]
fn not_today_hides_the_chip_and_discard_removes_the_stage() {
    let f = make(Start::Plain, true);
    f.click_chip();
    f.app().invoke_menu_picked(2);
    f.pump();
    assert!(!f.app().get_staged_chip_shown());
    f.at(1_000);
    f.at(100_000);
    assert!(
        f.restarts.borrow().is_empty(),
        "a hidden build never installs on its own"
    );

    let g = make(Start::Plain, true);
    g.click_chip();
    g.app().invoke_menu_picked(3);
    g.pump();
    g.settle();
    assert!(!g.app().get_staged_chip_shown());
    assert!(!g.layout.staged().exists(), "the staged folder is gone");
}

/// A temp install whose new build was swapped in and has just started (the guard said "installed").
fn new_build_started(layout: &Layout) -> Start {
    stage(layout, b"new exe");
    staged::swap(layout).unwrap();
    staged::on_start(layout)
}

#[test]
fn the_first_start_of_a_new_build_says_installed_and_confirms_after_twenty_seconds() {
    let f = make_with(new_build_started);
    assert!(
        !f.layout.staged().exists(),
        "the staged folder went at the start"
    );
    assert_eq!(f.app().get_staged_toast(), "Installed abc1234");
    assert_eq!(f.app().get_staged_toast_button(), "");
    assert!(!f.app().get_staged_chip_shown());
    f.at(8_000);
    assert_eq!(f.app().get_staged_toast(), "", "the notice goes after 8 s");
    assert!(
        f.layout.current().join("FlareStable.exe.old").exists(),
        "kept until 20 s"
    );
    f.h.advance(20_000);
    f.pump();
    f.settle();
    assert!(!f.layout.current().join("FlareStable.exe.old").exists());
    assert!(
        staged::read_marker(&f.layout).is_none(),
        "the build is good"
    );
    assert_eq!(f.read("FlareStable.exe"), b"new exe");
}

#[test]
fn a_new_build_that_quits_cleanly_before_twenty_seconds_counts_as_good() {
    let f = make_with(new_build_started);
    f.ctl.borrow_mut().end();
    assert!(staged::read_marker(&f.layout).is_none());
    assert!(!f.layout.current().join("FlareStable.exe.old").exists());
}

#[test]
fn a_new_build_that_dies_early_is_not_confirmed() {
    let f = make_with(new_build_started);
    // No 20 s and no clean quit (the process died): the record stays pending, so the next start counts it.
    f.at(19_000);
    let m = staged::read_marker(&f.layout).unwrap();
    assert_eq!(m.state, staged::MarkerState::Pending);
    assert_eq!(m.starts, 1);
}

#[test]
fn the_rolled_back_build_says_so_once() {
    let f = make(
        Start::RolledBack {
            commit: "abc1234def".into(),
        },
        false,
    );
    assert_eq!(
        f.app().get_staged_toast(),
        "Build abc1234 did not start, so the last build is back"
    );
}

#[test]
fn the_chip_is_drawn_in_the_bar_and_the_toast_hangs_under_it() {
    let f = make(Start::Plain, true);
    f.at(1_000);
    f.at(61_000);
    let app = f.app();
    let (cx, cy, cw, ch) = (
        app.get_staged_chip_x(),
        app.get_staged_chip_y(),
        app.get_staged_chip_w(),
        app.get_staged_chip_h(),
    );
    assert!(cw > 40.0 && ch == 24.0, "the chip has a size: {cw} x {ch}");
    assert!(cy >= 0.0 && cy + ch <= 36.0, "in the 36 px bar: {cy}");
    assert!(cx > 500.0, "at the right of the bar: {cx}");
    let (tx, ty, tw) = (
        app.get_staged_toast_x(),
        app.get_staged_toast_y(),
        app.get_staged_toast_w(),
    );
    assert!(
        (tx + tw - (cx + cw)).abs() < 1.0,
        "right-aligned to the chip"
    );
    assert!(ty >= cy + ch, "under the chip");
    assert!(
        app.get_overlay_open(),
        "the toast hides a web panel like the update notice"
    );
}

#[test]
fn install_now_with_no_agent_at_work_asks_when_one_starts_during_the_countdown() {
    let f = make(Start::Plain, true);
    f.click_chip();
    f.app().invoke_menu_picked(0);
    f.pump();
    assert!(matches!(f.phase(), Phase::Countdown { by_hand: true, .. }));
    f.agent_working(true);
    f.at(2_000);
    let app = f.app();
    assert!(app.get_dialog_open(), "the close question asks first");
    assert_eq!(f.phase(), Phase::Waiting);
    f.at(30_000);
    assert!(f.restarts.borrow().is_empty(), "nothing while it asks");
    app.invoke_dialog_confirm();
    f.pump();
    assert!(matches!(f.phase(), Phase::Countdown { by_hand: true, .. }));
    f.at(41_000);
    assert_eq!(f.restarts.borrow().len(), 1, "a yes goes ahead");
}

#[test]
fn an_agent_that_starts_while_the_swap_runs_takes_the_swap_back() {
    let f = make(Start::Plain, true);
    f.at(1_000);
    f.at(61_000);
    // The countdown ends: the swap is queued on a worker, and an agent starts before it is done.
    f.clock.set(71_000);
    f.h.advance(250);
    f.pump();
    assert_eq!(f.phase(), Phase::Installing);
    f.agent_working(true);
    f.settle();
    assert!(
        f.restarts.borrow().is_empty(),
        "no restart under a working agent"
    );
    assert_eq!(f.quits.get(), 0);
    assert_eq!(f.read("FlareStable.exe"), b"old exe");
    assert!(!f.layout.current().join("FlareStable.exe.old").exists());
    assert!(staged::read_marker(&f.layout).is_none());
    assert_eq!(f.phase(), Phase::Waiting);
    assert!(
        f.app().get_staged_chip_shown(),
        "the build is still on offer"
    );
    // Once the agent is done and a quiet minute passes, it installs.
    f.agent_working(false);
    f.at(80_000);
    f.at(140_000);
    f.at(150_250);
    assert_eq!(f.restarts.borrow().len(), 1);
    assert_eq!(f.read("FlareStable.exe"), b"new exe");
}

#[test]
fn a_new_build_not_yet_confirmed_can_be_replaced_and_the_one_it_ran_is_put_aside() {
    let f = make_with(new_build_started);
    stage(&f.layout, b"newer exe");
    f.changed();
    f.click_chip();
    f.app().invoke_menu_picked(0);
    f.pump();
    f.at(10_000);
    assert_eq!(
        f.restarts.borrow().len(),
        1,
        "{}",
        f.app().get_staged_toast()
    );
    assert_eq!(f.read("FlareStable.exe"), b"newer exe");
    assert_eq!(
        f.read("FlareStable.exe.old"),
        b"new exe",
        "the build it ran from is the one put aside"
    );
}
