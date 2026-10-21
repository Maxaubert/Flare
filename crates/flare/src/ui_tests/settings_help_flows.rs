//! UI flows of the command help (M7 Task 49); headless only.
//!
//! The whole window with its controllers over fake shells: the title button, F1, the chips, the
//! search, the copy, the keys, the grip and the switch. The geometry of the panel is in
//! `settings_help.rs`; these tests drive the same bridge the panel draws from, through `AppCtl`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_help::entries::is_key_press;
use flare_help::panel::{COPIED_MS, EMPTY_TEXT, FIRST_PAGE, NEXT_PAGE, PLACEHOLDER};
use flare_render::glyph::{FakeGlyphs, GlyphSource};
use flare_render::metrics::cell_metrics;
use flare_settings::help_prefs::{HELP_WIDTH, help_enabled, help_width, set_help_enabled};
use flare_settings::kv::PrefStore;
use flare_shells::list::ShellDef;
use flare_tabs::restore::{SavedTab, SavedTabs, tabs_json};
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
use flare_term::keys::KeyEvent;
use flare_term::path_open::FsStat;
use flare_win::clipboard_port::{ClipboardPort, MemoryClipboard};
use flare_win::dblclick::FALLBACK;
use flare_win::window_state::WindowStateStore;
use slint::{ComponentHandle, Model};

use crate::appctl::{AppCtl, Deps, KeyOutcome, Post, TermPorts, UiMsg};
use crate::openers::RecordingOpeners;
use crate::tab_host::TabHost;
use crate::ui_harness::Harness;
use crate::{AppWindow, HelpBridge};

const CWD: &str = "C:\\a";

#[derive(Default)]
struct Opts {
    shells: Option<Vec<ShellDef>>,
}

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Arc<Mutex<VecDeque<UiMsg>>>,
    sp: Arc<FakeSpawner>,
    prefs: PrefStore,
    clip: Arc<MemoryClipboard>,
    /// What the clipboard port was asked to take (the help copy goes through it).
    copied: Rc<RefCell<Vec<String>>>,
    _dir: tempfile::TempDir,
}

fn fx(opts: Opts) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let saved = SavedTabs {
        tabs: vec![SavedTab {
            cwd: CWD.to_string(),
            agent: None,
        }],
        active: 0,
    };
    std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test code only
    let prefs = PrefStore::memory();
    let q: Arc<Mutex<VecDeque<UiMsg>>> = Arc::default();
    let post: Post = {
        let q = q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let sp = FakeSpawner::new();
    let waker: Arc<dyn Fn() + Send + Sync> = {
        let post = post.clone();
        Arc::new(move || post(UiMsg::HostWake))
    };
    let shells = match opts.shells {
        Some(defs) => FakeShells::with(defs),
        None => FakeShells::new(),
    };
    let host = TabHost::new(sp.clone(), shells, prefs.clone(), waker, None);
    let clip = Arc::new(MemoryClipboard::new());
    let openers = Arc::new(RecordingOpeners::new());
    let copied: Rc<RefCell<Vec<String>>> = Rc::default();
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
        lookup: Arc::new(flare_tabs::restore::NoSessions),
        is_dir: Arc::new(|p| !p.contains("gone")),
        home: "C:\\home".to_string(),
        clock: Rc::new(|| 0u64),
        post,
        pick_folder: Rc::new(|_default, done| done(None)),
        clipboard: {
            let copied = Rc::clone(&copied);
            Rc::new(move |s| copied.borrow_mut().push(s.to_string()))
        },
        open_url: Rc::new(|_| {}),
        set_grid_source: Rc::new(|_| {}),
        raise: Rc::new(|| {}),
        quit: Rc::new(|| {}),
        hide: Rc::new(|| {}),
        launch_folders: Vec::new(),
        e2e: true,
        term: TermPorts {
            clip_port: clip.clone(),
            openers: openers.clone(),
            agent: None,
            dbl: FALLBACK,
            stat: Arc::new(FsStat),
            metrics: Rc::new(|| cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0)),
            mark_grid_dirty: Rc::new(|| {}),
        },
    };
    let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
    let ctl = AppCtl::new(&h.comp, deps);
    // Pinned: the runner's Windows "show animations" switch must not decide a test.
    ctl.borrow_mut().set_animations_source(Rc::new(|| true));
    Fx {
        h,
        ctl,
        q,
        sp,
        prefs,
        clip,
        copied,
        _dir: dir,
    }
}

/// One live shell tab (`C:\a`) restored at start, its fake pty.
fn live(opts: Opts) -> (Fx, Arc<FakeState>) {
    let f = fx(opts);
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    f.pump_until("the spawn", |f| !f.made_with().is_empty());
    let fake = f.made_with()[0].clone();
    f.pump_until("the shell", |f| {
        f.ctl
            .borrow()
            .handle_of(&f.front_id())
            .is_some_and(|h| h.shell_id().is_some())
    });
    (f, fake)
}

fn f1() -> KeyEvent {
    KeyEvent::new("F1", "F1")
}

fn ctrl(key: &str, code: &str) -> KeyEvent {
    KeyEvent::new(key, code).ctrl()
}

fn cell_css(col: usize, row: usize) -> (f32, f32) {
    let m = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0);
    let x = m.padding as f32 + col as f32 * m.cell_w as f32 + 0.25 * m.cell_w as f32;
    let y = m.padding as f32 + row as f32 * m.cell_h as f32 + m.cell_h as f32 / 2.0;
    (x, y)
}

impl Fx {
    fn pump(&self) {
        loop {
            let m = self.q.lock().unwrap().pop_front();
            let Some(m) = m else { break };
            self.ctl.borrow_mut().on_msg(m);
        }
    }

    fn pump_until(&self, what: &str, mut cond: impl FnMut(&Fx) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            self.pump();
            if cond(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn front_id(&self) -> String {
        self.ctl
            .borrow()
            .model()
            .state
            .tabs
            .first()
            .map(|t| t.id.clone())
            .unwrap_or_default()
    }

    fn made_with(&self) -> Vec<Arc<FakeState>> {
        self.sp
            .made()
            .into_iter()
            .filter(|m| m.spec().cwd == CWD)
            .collect()
    }

    fn key(&self, ev: KeyEvent) -> KeyOutcome {
        self.ctl.borrow_mut().on_key(&ev)
    }

    fn tabs(&self) -> usize {
        self.h.comp.get_tab_rows().row_count()
    }

    fn bridge(&self) -> HelpBridge<'_> {
        self.h.comp.global::<HelpBridge>()
    }

    fn open(&self) {
        self.h.comp.invoke_help_clicked();
        self.h.advance(1);
        self.h.advance(900);
    }

    fn is_open(&self) -> bool {
        self.bridge().get_open()
    }

    fn row(&self, i: usize) -> crate::HelpRowData {
        self.bridge().get_rows().row_data(i).expect("a row")
    }

    /// The index of the first row whose command is `cmd`.
    fn row_of(&self, cmd: &str) -> usize {
        self.bridge()
            .get_rows()
            .iter()
            .position(|r| r.command == cmd)
            .unwrap_or_else(|| panic!("no row with {cmd}"))
    }

    fn chip_on(&self) -> Vec<String> {
        self.bridge()
            .get_chips()
            .iter()
            .filter(|c| c.on)
            .map(|c| c.id.to_string())
            .collect()
    }

    fn ptr(&self, kind: i32, button: i32, at: (f32, f32)) {
        self.h
            .comp
            .invoke_grid_pointer(kind, button, at.0, at.1, false, false, false);
    }
}

#[test]
fn the_title_button_and_f1_open_the_panel_and_focus_the_field() {
    let (f, _) = live(Opts::default());
    f.h.comp.invoke_help_clicked();
    f.h.advance(1);
    f.h.advance(900);
    assert!(f.is_open(), "the title button opens it");
    f.bridge().invoke_closed();
    f.h.advance(OUT_MS_PLUS);
    assert!(!f.is_open());
    assert_eq!(f.key(f1()), KeyOutcome::Consumed, "F1 is the app's chord");
    assert!(f.is_open(), "F1 opens it over the focused shell");
    f.h.advance(1);
    f.h.advance(80);
    assert!(f.h.comp.get_find_input_focused(), "the field has the keys");
}

/// The panel's slide-out plus one frame (OUT_MS is 220).
const OUT_MS_PLUS: u64 = 240;

#[test]
fn the_chip_follows_the_front_tabs_shell() {
    let cmd = ShellDef {
        id: "cmd".to_string(),
        name: "Command Prompt".to_string(),
        exe: "cmd.exe".to_string(),
        args: Vec::new(),
    };
    let (f, _) = live(Opts {
        shells: Some(vec![cmd]),
    });
    f.open();
    assert_eq!(f.chip_on(), ["cmd"], "the front tab is Command Prompt");
}

#[test]
fn browsing_lists_the_categories_in_order_with_20_to_200_rows_first() {
    let (f, _) = live(Opts::default());
    f.open();
    let n = f.bridge().get_rows().row_count();
    assert_eq!(n, FIRST_PAGE, "the first page is 40 rows");
    assert!(
        !f.row(0).header.is_empty(),
        "the first row starts a category heading"
    );
    f.bridge().invoke_near_end();
    let more = f.bridge().get_rows().row_count();
    assert_eq!(more, FIRST_PAGE + NEXT_PAGE, "the next page is 60 rows");
}

#[test]
fn the_field_says_try_find_big_files_what_is_using_port_3000() {
    let (f, _) = live(Opts::default());
    assert_eq!(f.bridge().get_placeholder(), PLACEHOLDER);
    assert!(PLACEHOLDER.contains("find big files"));
    assert!(PLACEHOLDER.contains("port 3000"));
}

#[test]
fn the_search_field_is_named_by_the_long_search_label() {
    let (f, _) = live(Opts::default());
    assert_eq!(
        f.bridge().get_search_label(),
        flare_help::panel::SEARCH_LABEL
    );
}

#[test]
fn find_big_files_finds_the_shell_specific_answer() {
    let (f, _) = live(Opts::default());
    f.open();
    f.bridge().invoke_query_edited("find big files".into());
    assert!(
        f.row(0).key.starts_with("ps-biggest-files#"),
        "PowerShell first: {}",
        f.row(0).key
    );
    f.bridge().invoke_chip_picked("bash".into());
    assert!(
        f.row(0).key.starts_with("sh-biggest-files#"),
        "Bash changes the answer: {}",
        f.row(0).key
    );
}

#[test]
fn copy_draws_a_green_check_for_1400_ms_and_no_copied_badge() {
    let (f, _) = live(Opts::default());
    f.open();
    let i = f.row_of("Get-ChildItem");
    f.bridge().invoke_copy_clicked(i as i32);
    assert_eq!(f.copied.borrow().as_slice(), ["Get-ChildItem"]);
    assert!(f.row(i).copied, "the check is drawn");
    // The copied check is timed on the real clock; the slint timer fires on the mock one.
    std::thread::sleep(Duration::from_millis(COPIED_MS + 60));
    f.h.advance(COPIED_MS + 10);
    f.pump();
    assert!(!f.row(i).copied, "the check leaves after 1400 ms");
    assert_eq!(f.copied.borrow().len(), 1, "one copy, not two");
}

#[test]
fn no_write_reaches_a_shell_across_search_copy_and_enter() {
    let (f, fake) = live(Opts::default());
    f.open();
    f.bridge().invoke_query_edited("find big files".into());
    f.bridge().invoke_copy_clicked(0);
    for k in ["Down", "Up", "Enter", "PageDown"] {
        f.bridge().invoke_key(k.into(), false, false);
    }
    assert!(
        fake.written().is_empty(),
        "rule 82: no byte reaches a shell"
    );
}

#[test]
fn escape_closes_and_chords_reach_the_host() {
    let (f, _) = live(Opts::default());
    f.open();
    f.bridge().invoke_key("Escape".into(), false, false);
    f.h.advance(OUT_MS_PLUS);
    assert!(!f.is_open(), "Escape closes the panel");
    f.open();
    let before = f.tabs();
    assert_eq!(
        f.key(ctrl("t", "t")),
        KeyOutcome::Consumed,
        "Ctrl+T is the app's"
    );
    assert_eq!(
        f.tabs(),
        before + 1,
        "a new tab opened while the panel had focus"
    );
}

#[test]
fn the_grip_resizes_by_drag_and_keys_and_remembers() {
    let (f, _) = live(Opts::default());
    f.open();
    f.bridge().set_grip_from(620.0);
    f.bridge().invoke_grip_dragged(-200.0);
    assert_eq!(help_width(&f.prefs), 420, "clamped at the minimum");
    f.bridge().set_grip_from(620.0);
    f.bridge().invoke_grip_dragged(1000.0);
    assert_eq!(
        help_width(&f.prefs),
        HELP_WIDTH.max,
        "clamped at the maximum"
    );
    // The keys step from the width on show, which the window keeps inside its container less 120.
    let shown = f.h.comp.get_help_sheet_w();
    f.bridge().invoke_grip_key("Right".into(), false);
    assert_eq!(
        help_width(&f.prefs),
        (shown - 20.0) as u32,
        "a right key narrows by 20"
    );
    f.bridge().invoke_grip_key("Left".into(), false);
    assert_eq!(
        help_width(&f.prefs),
        shown as u32,
        "a left key widens by 20 again"
    );
}

#[test]
fn the_panel_stays_across_a_tab_switch() {
    let (f, _) = live(Opts::default());
    f.open();
    f.h.comp.set_front_kind(1);
    f.h.advance(50);
    f.h.comp.set_front_kind(0);
    f.h.advance(50);
    assert!(f.is_open(), "the panel keeps its place");
}

#[test]
fn a_key_row_has_caps_and_copies_nothing() {
    let (f, _) = live(Opts::default());
    f.open();
    let mut found = None;
    for _ in 0..10 {
        if let Some(i) = f.bridge().get_rows().iter().position(|r| r.keys) {
            found = Some(i);
            break;
        }
        f.bridge().invoke_near_end();
    }
    let i = found.expect("a key row in the catalogue");
    assert!(is_key_press(&f.row(i).command), "a key press row");
    f.bridge().invoke_row_clicked(i as i32, 0.0);
    assert!(f.copied.borrow().is_empty(), "a key row copies nothing");
}

#[test]
fn no_match_says_how_to_ask_again() {
    let (f, _) = live(Opts::default());
    f.open();
    f.bridge().invoke_query_edited("zzqxqzzy".into());
    assert_eq!(f.bridge().get_rows().row_count(), 0);
    assert!(f.bridge().get_empty(), "the empty line shows");
    assert_eq!(f.bridge().get_empty_text(), EMPTY_TEXT);
    assert!(f.clip.read_text().is_none(), "nothing was copied");
}

#[test]
fn with_help_off_there_is_no_button_no_f1_and_no_menu_row() {
    let (f, _) = live(Opts::default());
    assert!(f.h.comp.get_help_visible());
    set_help_enabled(&f.prefs, false);
    f.pump();
    assert!(!help_enabled(&f.prefs));
    assert!(!f.h.comp.get_help_visible(), "no title button");
    assert_eq!(f.key(f1()), KeyOutcome::Pty, "F1 is the shell's now");
    assert!(!f.is_open());
    // The terminal menu has no Command help row.
    f.ptr(0, 2, cell_css(2, 1));
    f.ptr(1, 2, cell_css(2, 1));
    assert!(f.h.comp.get_menu_open());
    let rows = f.h.comp.get_menu_rows();
    assert!(rows.iter().all(|r| r.label != "Command help"));
}
