//! M9 Task 34: the update flows, the guard and the hostile notes. Headless only.
//!
//! One `Harness<AppWindow>` plus the real `AppCtl` (with its `UpdateCtl`, wired by
//! `AppCtl::attach_update`) and a real `Engine` over `FakeFeed` or `RefusingFeed`, per test. Nothing
//! starts a shell, a network source or an OS window. The test names carry the PT e2e scenarios they
//! port (`updateWindow`, `updateGuard`, `updateNotes`, `tools/e2e/run.mjs` in Prism Terminal v0.55.6).
//! The window's geometry is read from `UpdateWindowPreview` (its out properties are the layout), the
//! flows from the `AppWindow` the app itself drives.
//!
//! The test names are the PT e2e scenario names (camelCase where PT used them), so `docs/test-port.md`
//! can find each one by name; that is why the lint for snake_case names is off in this file.
#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flare_agent::kinds::AgentKind;
use flare_settings::kv::PrefStore;
use flare_tabs::restore::NoSessions;
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_term::keys::KeyEvent;
use flare_update::copy::{pct_text, status_line};
use flare_update::engine::{Calls, CondvarSleeper, Engine, Mode};
use flare_update::feed::{FakeFeed, Feed, RefusingFeed};
use flare_update::flow::{INSTALL_FAILED, PREVIEW_DONE};
use flare_update::notes::{MAX_ENTRIES, NO_NOTES};
use flare_update::preview::{SAMPLE_NOTES, Waiter};
use flare_update::types::{Offer, Phase};
use flare_win::window_state::WindowStateStore;
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::appctl::{AppCtl, CloseDecision, Deps, KeyOutcome, Post, TermPorts, UiMsg, UpdateParts};
use crate::tab_host::TabHost;
use crate::ui_harness::Harness;
use crate::update_ctl::PostEvents;
use crate::{AppWindow, NoteSection, UpdateWindowPreview};

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;
type Jobs = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;

/// `Theme.sel-bg` (the chip fill) and `Theme.on-accent` (the chip's label ink), as in the chip tests.
const SEL_BG: [u8; 4] = [0xfe, 0x8f, 0x34, 255];
const ON_ACCENT: [u8; 4] = [0x0b, 0x0b, 0x0f, 255];
const ESC: &str = "\u{1b}";

/// The preview's current version: its offer is the next minor, 0.56.0.
const CURRENT: &str = "0.55.6";

struct NoWait;

impl Waiter for NoWait {
    fn wait(&self, _ms: u64) {}
}

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Queue,
    engine: Arc<Engine>,
    jobs: Jobs,
    quits: Rc<Cell<u32>>,
    raised: Rc<Cell<u32>>,
    clock: Rc<Cell<u64>>,
    /// Counts the downloads the feed was asked for (shares its state with the feed the engine uses).
    probe: FakeFeed,
    _dir: tempfile::TempDir,
}

fn make(mode: Mode, feed: Arc<dyn Feed>, probe: FakeFeed, current: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let q: Queue = Arc::default();
    let post: Post = {
        let q = q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let prefs = PrefStore::memory();
    let sp = FakeSpawner::new();
    let waker: Arc<dyn Fn() + Send + Sync> = {
        let post = post.clone();
        Arc::new(move || post(UiMsg::HostWake))
    };
    let host = TabHost::new(sp.clone(), FakeShells::new(), prefs.clone(), waker, None);
    let clock = Rc::new(Cell::new(0u64));
    let quits = Rc::new(Cell::new(0u32));
    let raised = Rc::new(Cell::new(0u32));
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
        is_dir: Arc::new(|p| !p.contains("gone")),
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
        raise: {
            let r = raised.clone();
            Rc::new(move || r.set(r.get() + 1))
        },
        quit: {
            let k = quits.clone();
            Rc::new(move || k.set(k.get() + 1))
        },
        hide: Rc::new(|| {}),
        launch_folders: Vec::new(),
        e2e: true,
        term: TermPorts::inert(),
    };
    let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
    let ctl = AppCtl::new(&h.comp, deps);
    // Pinned: the runner's Windows "show animations" switch must not decide a test.
    ctl.borrow_mut().set_animations_source(Rc::new(|| true));
    h.advance(0);
    ctl.borrow_mut().refresh();
    let engine = Engine::new(
        mode,
        feed,
        current,
        None,
        Arc::new(PostEvents { post: post.clone() }),
        Arc::new(CondvarSleeper::new()),
        Arc::new(NoWait),
    );
    let jobs: Jobs = Rc::default();
    {
        let j = jobs.clone();
        AppCtl::attach_update(
            &h.comp,
            &ctl,
            UpdateParts {
                engine: engine.clone(),
                current_version: current.to_string(),
                e2e_dir: None,
                spawn: Rc::new(move |job| j.borrow_mut().push(job)),
            },
        );
    }
    Fx {
        h,
        ctl,
        q,
        engine,
        jobs,
        quits,
        raised,
        clock,
        probe,
        _dir: dir,
    }
}

/// A live engine over a fake feed that offers `version` with `notes`.
fn offering(version: &str, notes: &str) -> Fx {
    let probe = FakeFeed::new().offering(version, notes);
    make(Mode::Live, Arc::new(probe.clone()), probe, CURRENT)
}

impl Fx {
    /// Applies every message the workers and the engine posted, on this (the UI) thread.
    fn pump(&self) {
        loop {
            let m = self.q.lock().unwrap().pop_front();
            let Some(m) = m else { break };
            self.ctl.borrow_mut().on_msg(m);
        }
    }

    /// Runs the queued worker jobs (the installs and the state writes) on this thread.
    fn run_jobs(&self) {
        let jobs: Vec<_> = self.jobs.borrow_mut().drain(..).collect();
        for j in jobs {
            j();
        }
    }

    /// One engine check, then the messages it posted.
    fn offered(&self) {
        self.engine.tick();
        self.pump();
    }

    fn app(&self) -> &AppWindow {
        &self.h.comp
    }

    fn click_chip(&self) {
        self.h.comp.invoke_update_chip_clicked();
        self.pump();
    }

    fn install(&self) {
        self.h.comp.invoke_update_install();
        self.pump();
    }

    fn rows(&self) -> usize {
        self.h.comp.get_tab_rows().row_count()
    }

    fn key(&self, ev: KeyEvent) -> KeyOutcome {
        self.ctl.borrow_mut().on_key(&ev)
    }
}

fn notes_of(app: &AppWindow) -> Vec<(String, Vec<String>)> {
    app.get_update_sections()
        .iter()
        .map(|s: NoteSection| {
            (
                s.heading.to_string(),
                s.entries.iter().map(|e| e.to_string()).collect(),
            )
        })
        .collect()
}

fn entries_of(app: &AppWindow) -> Vec<String> {
    notes_of(app).into_iter().flat_map(|(_, e)| e).collect()
}

/// The hostile body of PT's `updateNotes` scenario, plus a bidi override and a `javascript:` link.
fn hostile_body() -> String {
    [
        "## What's Changed",
        "* <img src=x onerror=\"window.__pwned = 1\"> an image tag by @a in https://github.com/o/r/pull/1",
        "* [a link](https://evil.example/login) and <a href=\"https://evil.example\">an anchor</a> by @a in https://github.com/o/r/pull/2",
        "* <script>window.__pwned = 2</script> a script by @a in https://github.com/o/r/pull/3",
        "* <iframe src=\"https://evil.example\"></iframe> a frame by @a in https://github.com/o/r/pull/4",
        "* [click me](javascript:alert(1)) a \u{202e}reversed\u{202c} line by @a in https://github.com/o/r/pull/5",
        "",
        "**Full Changelog**: https://github.com/o/r/compare/v1...v2",
    ]
    .join("\n")
}

fn long_body(n: usize) -> String {
    let mut lines = vec!["## What's Changed".to_string()];
    for i in 0..n {
        lines.push(format!(
            "* Change number {i} with a title long enough to wrap onto a second line by @a in https://github.com/o/r/pull/{i}"
        ));
    }
    lines.join("\n")
}

fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
    let hex = |c: [u8; 4]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    flare_theme::ansi::contrast_ratio(&hex(a), &hex(b))
}

// ---- the preview -----------------------------------------------------------------------------

#[test]
fn updateWindow_preview_chip_next_minor_accent_click_opens_nothing_installs() {
    let f = make(
        Mode::Preview,
        Arc::new(FakeFeed::new()),
        FakeFeed::new(),
        CURRENT,
    );
    f.engine.start();
    f.pump();
    let app = f.app();
    assert!(app.get_update_chip_shown(), "the preview chip is shown");
    assert_eq!(app.get_update_chip_label(), "Update 0.56.0");
    assert!(app.get_update_chip_tip().starts_with("Preview: see what"));
    assert!(
        contrast(SEL_BG, ON_ACCENT) >= 4.5,
        "the chip's label on its fill reads at 4.5:1 or better"
    );

    f.click_chip();
    assert!(app.get_update_open(), "the click opens the window");
    assert!(f.jobs.borrow().is_empty(), "and starts nothing");
    assert_eq!(app.get_update_title(), "Update to 0.56.0");
    assert_eq!(app.get_update_sub(), "You have 0.55.6");

    let sections = notes_of(app);
    let headings: Vec<&str> = sections.iter().map(|(h, _)| h.as_str()).collect();
    assert_eq!(
        headings,
        ["New features", "Bug fixes", "Under the hood"],
        "the sections come in reading order"
    );
    let entries = entries_of(app);
    assert!(entries.len() >= 5, "at least five entries: {entries:?}");
    for e in &entries {
        for banned in [
            "by @",
            "http",
            "Full Changelog",
            "New Contributors",
            "What's Changed",
            "fix:",
            "fix(",
            "feat:",
        ] {
            assert!(!e.contains(banned), "{banned:?} in {e:?}");
        }
        assert!(!e.contains("pull/"), "no PR link in {e:?}");
    }

    f.install();
    f.run_jobs();
    f.pump();
    assert_eq!(app.get_update_status(), PREVIEW_DONE);
    assert_eq!(f.engine.calls(), Calls::default(), "no check, no install");
}

#[test]
fn updateWindow_the_box_never_changes_size_while_the_preview_runs() {
    let h = Harness::new(1.0, 1000, 640, || {
        UpdateWindowPreview::new().expect("the preview builds")
    });
    h.comp.set_window_title("Update to 0.56.0".into());
    h.comp.set_current_text("You have 0.55.6".into());
    h.comp
        .set_sections(ModelRc::from(Rc::new(VecModel::from(vec![NoteSection {
            heading: "New features".into(),
            entries: ModelRc::from(Rc::new(VecModel::from(vec![SharedString::from(
                "A faster start",
            )]))),
        }]))));
    h.comp.set_notes_max_h(0.44 * 640.0);
    h.comp.set_open(true);
    h.advance(0);
    let _ = h.render();
    let base = (h.comp.get_box_w(), h.comp.get_box_h());
    h.comp.set_active(true);
    h.comp.set_running(true);
    // Every 25 ms of the scene clock, through the whole preview: 0 to 100 percent and the ending.
    for step in 0..=24u32 {
        let pct = (step * 5).min(100) as i32;
        h.comp.set_pct(pct);
        h.comp.set_pct_text(pct_text(pct as u8).into());
        h.comp
            .set_status(status_line(Phase::Downloading, true, false, None).into());
        h.advance(25);
        assert_eq!(
            (h.comp.get_box_w(), h.comp.get_box_h()),
            base,
            "at {pct} percent"
        );
    }
    h.comp.set_status(PREVIEW_DONE.into());
    h.comp.set_running(false);
    h.advance(25);
    assert_eq!(
        (h.comp.get_box_w(), h.comp.get_box_h()),
        base,
        "at the ending"
    );
}

// ---- the guard -------------------------------------------------------------------------------

#[test]
fn updateGuard_a_real_shaped_offer_shows_the_chip_and_is_not_a_preview() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    let app = f.app();
    assert!(app.get_update_chip_shown());
    assert_eq!(app.get_update_chip_label(), "Update 99.0.0");
    assert_eq!(app.get_update_chip_tip(), "See what is new in 99.0.0");
    assert!(
        !f.engine.offer().expect("an offer").mock,
        "a real offer, not a mock"
    );
    assert!(!app.get_update_open(), "the chip alone opens nothing");
}

#[test]
fn updateGuard_install_for_another_token_is_refused_with_no_fetch() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    let token = f.engine.offer().expect("an offer").token;
    assert!(
        !f.engine.install(Some(token + 1), &|| {}),
        "a token that is not the offer's is refused"
    );
    assert_eq!(f.probe.downloads(), 0, "and nothing was fetched");
    assert_eq!(f.engine.calls().installs, 0);
}

#[test]
fn updateGuard_empty_notes_say_no_notes_were_published() {
    let f = offering("99.0.0", "");
    f.offered();
    f.click_chip();
    assert!(f.app().get_update_open());
    assert_eq!(f.app().get_update_empty_note(), NO_NOTES);
    assert_eq!(f.app().get_update_sections().row_count(), 0);
}

#[test]
fn updateGuard_ctrl_w_over_the_update_window_still_asks_about_the_agent() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    f.click_chip();
    let id = f.ctl.borrow_mut().open_folder("C:\\a");
    {
        let mut c = f.ctl.borrow_mut();
        c.model_mut().set_agent(&id, Some(AgentKind::Claude));
        c.set_working(&id, Some(0));
    }
    f.clock.set(5_000);
    // Ctrl+W over the update window still raises the question about the agent; no tab closes.
    assert_eq!(
        f.key(KeyEvent::new("w", "KeyW").ctrl()),
        KeyOutcome::Consumed
    );
    assert!(f.app().get_dialog_open(), "the question is up");
    assert_eq!(
        f.app().get_dialog_title().as_str(),
        "Stop the agent and close?"
    );
    assert_eq!(f.rows(), 1, "no tab closed behind the question");
    // Escape reaches the question through the window's own keys (the controller hands it on).
    assert_eq!(
        f.key(KeyEvent::new("Escape", "Escape")),
        KeyOutcome::Slint,
        "Escape goes to the question, not the shell"
    );
    f.app().invoke_dialog_cancel();
    f.pump();
    assert!(
        !f.app().get_dialog_open(),
        "Escape backs out of the question"
    );
    assert!(
        f.app().get_update_open(),
        "and the update window is still there"
    );
    assert_eq!(f.rows(), 1);

    // The window's own close is held the same way while an agent works.
    assert_eq!(
        f.ctl.borrow_mut().on_close_requested(),
        CloseDecision::KeepOpen,
        "an agent works: the close is held"
    );
    assert!(f.app().get_dialog_open(), "the question is up");
    assert_eq!(
        f.app().get_dialog_title().as_str(),
        "Stop the agent and close the window?"
    );
    assert_eq!(f.raised.get(), 1, "the window is raised for it");
    f.app().invoke_dialog_cancel();
    f.pump();
    assert!(
        !f.app().get_dialog_open(),
        "backing out closes the question"
    );
    assert_eq!(f.quits.get(), 0);
}

#[test]
fn updateGuard_working_agent_install_asks_stop_the_agent_and_install_the_update() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    let id = f.ctl.borrow_mut().open_folder("C:\\a");
    {
        let mut c = f.ctl.borrow_mut();
        c.model_mut().set_agent(&id, Some(AgentKind::Claude));
        c.set_working(&id, Some(0));
    }
    f.clock.set(5_000);
    f.click_chip();
    f.install();
    let app = f.app();
    assert!(app.get_dialog_open(), "the close question is up");
    assert_eq!(
        app.get_dialog_title().as_str(),
        "Stop the agent and install the update?"
    );
    assert_eq!(app.get_dialog_primary().as_str(), "Install and restart");
    assert!(!app.get_update_open(), "the update window steps aside");
    assert!(f.jobs.borrow().is_empty(), "nothing starts while asked");
    assert_eq!(f.engine.calls().installs, 0);

    app.invoke_dialog_cancel();
    f.pump();
    assert!(!app.get_dialog_open());
    assert!(app.get_update_open(), "Cancel: the window comes back");
    assert!(f.jobs.borrow().is_empty(), "and nothing started");
    assert_eq!(f.engine.calls().installs, 0);

    f.install();
    app.invoke_dialog_confirm();
    f.pump();
    assert_eq!(f.jobs.borrow().len(), 1, "the go-ahead starts the install");
    assert!(app.get_update_running());
    assert_eq!(
        f.quits.get(),
        0,
        "the app quits later, when the installer starts"
    );
}

#[test]
fn updateGuard_a_failed_download_shows_could_not_be_downloaded_once_and_the_chip_offers_again() {
    let refusing: Arc<dyn Feed> = Arc::new(RefusingFeed::new(SAMPLE_NOTES));
    let f = make(Mode::Off, refusing, FakeFeed::new(), CURRENT);
    f.engine.set_offer(Offer {
        version: "99.0.0".into(),
        notes: SAMPLE_NOTES.into(),
        mock: false,
        token: 0,
    });
    f.pump();
    f.click_chip();
    f.install();
    f.run_jobs();
    f.pump();
    let app = f.app();
    assert_eq!(app.get_update_status(), INSTALL_FAILED);
    assert_eq!(app.get_update_notice(), "", "said once, in the window");
    assert!(app.get_update_chip_shown(), "the chip offers again");
    assert!(app.get_update_install_enabled(), "Install is offered again");
    assert_eq!(app.get_update_cancel_label(), "Close");
}

// ---- the hostile notes -----------------------------------------------------------------------

#[test]
fn updateNotes_a_hostile_body_becomes_no_element_and_runs_no_handler() {
    let f = offering("99.0.0", &hostile_body());
    f.offered();
    f.click_chip();
    let app = f.app();
    assert!(app.get_update_open());
    let entries = entries_of(app);
    assert!(!entries.is_empty(), "the lines survive as plain strings");
    let text = entries.join("|");
    for banned in [
        "<",
        ">",
        "onerror",
        "javascript:",
        "evil.example",
        "by @",
        "http",
        "pwned",
        "\u{202e}",
    ] {
        assert!(!text.contains(banned), "{banned:?} survived in {text:?}");
    }
    assert!(
        text.contains("An image tag") && text.contains("A link and an anchor"),
        "what is left of each line is its words: {text:?}"
    );
}

#[test]
fn updateNotes_the_list_is_capped_at_20_with_plus_10_more_and_scrolls_inside() {
    let f = offering("99.0.0", &long_body(30));
    f.offered();
    f.click_chip();
    let app = f.app();
    let shown = entries_of(app).len();
    assert_eq!(shown, MAX_ENTRIES, "20 shown");
    assert_eq!(app.get_update_more(), "+ 10 more");

    // The buttons stay on screen at 1000 x 420, with the list scrolling inside its cap.
    let h = Harness::new(1.0, 1000, 420, || {
        UpdateWindowPreview::new().expect("the preview builds")
    });
    h.comp.set_window_title("Update to 99.0.0".into());
    h.comp.set_current_text("You have 0.55.6".into());
    h.comp.set_sections(app.get_update_sections());
    h.comp.set_more_text("+ 10 more".into());
    h.comp
        .set_notes_max_h(flare_update::geom::notes_max_h(420.0));
    h.comp.set_open(true);
    h.advance(0);
    let _ = h.render();
    let on_screen = |y: f32, hh: f32| y >= 0.0 && y + hh <= 420.0;
    assert!(
        on_screen(h.comp.get_install_y(), h.comp.get_install_h()),
        "Install on screen"
    );
    assert!(
        on_screen(h.comp.get_cancel_y(), h.comp.get_cancel_h()),
        "Cancel on screen"
    );
    assert!(
        h.comp.get_notes_h() <= flare_update::geom::notes_max_h(420.0) + 0.01,
        "the list is capped inside the window"
    );
}

// ---- the keys ---------------------------------------------------------------------------------

#[test]
fn escape_backs_out_of_the_window_only_when_idle() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    f.click_chip();
    assert!(f.app().get_update_open());
    f.install();
    assert!(f.app().get_update_running(), "the install is running");
    f.h.key_text(ESC);
    assert!(
        f.app().get_update_open(),
        "a running install is not backed out of"
    );
    assert!(f.app().get_update_running());

    let g = offering("99.0.0", SAMPLE_NOTES);
    g.offered();
    g.click_chip();
    g.h.key_text(ESC);
    assert!(!g.app().get_update_open(), "idle, Escape closes the window");
    assert!(g.jobs.borrow().is_empty());
}

#[test]
fn a_second_chip_click_while_open_changes_nothing() {
    let f = offering("99.0.0", SAMPLE_NOTES);
    f.offered();
    f.click_chip();
    let app = f.app();
    let before = (
        app.get_update_title().to_string(),
        app.get_update_sub().to_string(),
        notes_of(app),
        f.engine.calls(),
    );
    f.click_chip();
    let after = (
        app.get_update_title().to_string(),
        app.get_update_sub().to_string(),
        notes_of(app),
        f.engine.calls(),
    );
    assert!(app.get_update_open());
    assert_eq!(before, after);
    assert!(f.jobs.borrow().is_empty());
}
