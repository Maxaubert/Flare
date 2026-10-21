//! Every Settings row takes effect on the live window while Settings stays open (owner, 2026-10-21:
//! "if i go from vertical to horizontal tabs it doesnt change before i click out of settings").
//! Prism Terminal applies each setting at once; so does Flare. Headless only.
//!
//! One `AppCtl` with the real `SettingsCtl`, the real look controller and the real font controller
//! over one in-memory store, as the window glue wires them. Each row is changed through the bridge
//! callback its control fires, with the Settings tab in front, and the main window's own state is
//! read before Settings is closed. The queue is pumped as the event loop would; the look's one
//! coalesced turn is run by the pump too (the harness has no event loop). One test per row class:
//!
//! - the window's own rows, which the tab view reads on each refresh (tab position, the keys);
//! - the look rows (theme, colours, panel edges), worn by the look controller;
//! - the terminal font rows, worn by the font controller;
//! - command help;
//! - the Diagnostics rows, which act on the log at once;
//! - the marks and the taskbar count live in `agent_flows.rs`, next to the agent signals they need;
//! - the Dictation rows are the dictation controller's own (it listens to its keys itself, Settings
//!   open or not); the e2e `dictation` switches it on from the page and sees the engine start with
//!   the page still in front.
//!
//! Rows Prism Terminal applies only later stay that way: the default shell and the exact status
//! from Claude Code (new terminals), the folder for new tabs (the next new tab).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::tab_layout::{TabLayout, tab_layout};
use flare_settings::verb::{DiagControlApi, FakeDiag};
use flare_tabs::restore::NoSessions;
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_win::window_state::WindowStateStore;
use slint::{ComponentHandle, Global};

use crate::appctl::{AppCtl, Deps, Post, TermPorts, UiMsg};
use crate::backend::BackendOutcome;
use crate::config::{LookOverrides, RendererChoice};
use crate::font_ctl::FontCtl;
use crate::host::{GridHost, GridPainter};
use crate::look_ctl::LookCtl;
use crate::settings_ctl::SettingsCtl;
use crate::tab_host::TabHost;
use crate::ui_harness::{Harness, SCALES};
use crate::{AppWindow, SettingsBridge, Theme};

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

/// The front kind of the Settings tab.
const SETTINGS: i32 = 1;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Queue,
    prefs: PrefStore,
    look_due: Arc<AtomicBool>,
    grid: Rc<RefCell<GridHost>>,
    scale: f32,
    diag: Rc<FakeDiag>,
    _settings: Rc<RefCell<SettingsCtl>>,
    _dir: tempfile::TempDir,
}

impl Drop for Fx {
    fn drop(&mut self) {
        LookCtl::uninstall();
    }
}

impl Fx {
    fn new(scale: f32) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let prefs = PrefStore::memory();
        crate::settings_scene::prep(&prefs);
        let q: Queue = Arc::default();
        let post: Post = {
            let q = q.clone();
            Arc::new(move |m| q.lock().unwrap().push_back(m))
        };
        let sp = FakeSpawner::new();
        let waker: Arc<dyn Fn() + Send + Sync> = {
            let post = post.clone();
            Arc::new(move || post(UiMsg::HostWake))
        };
        let host = TabHost::new(sp, FakeShells::new(), prefs.clone(), waker, None);
        let clock = Rc::new(Cell::new(1_000_u64));
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
            raise: Rc::new(|| {}),
            quit: Rc::new(|| {}),
            hide: Rc::new(|| {}),
            launch_folders: Vec::new(),
            e2e: true,
            term: TermPorts::inert(),
        };
        let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
        let ctl = AppCtl::new(&h.comp, deps);
        // Pinned: the runner's Windows "show animations" switch must not decide the clock or the marks (the help panel and chrome motion still read the OS).
        ctl.borrow_mut().set_animations_source(Rc::new(|| true));
        // The look and the font, as the window glue makes them over the same store.
        let grid = Rc::new(RefCell::new(GridHost::new(
            Box::new(flare_render::glyph::FakeGlyphs::new(8, 16)),
            GridPainter::Cpu,
        )));
        grid.borrow_mut()
            .resize((648.0 * scale) as u32, (264.0 * scale) as u32, scale);
        let look_due = Arc::new(AtomicBool::new(false));
        let outcome = BackendOutcome {
            effective: RendererChoice::Software,
            fallback_reason: None,
            glass_possible: false,
        };
        let look = LookCtl::new(
            &h.comp,
            LookStore::new(prefs.clone()),
            grid.clone(),
            Rc::new(RefCell::new(None)),
            &outcome,
            LookOverrides::default(),
        )
        .with_scheduler({
            let due = look_due.clone();
            Arc::new(move || due.store(true, Ordering::SeqCst))
        })
        .install();
        look.borrow_mut().apply_now();
        ctl.borrow_mut()
            .set_font_ctl(FontCtl::new(LookStore::new(prefs.clone()), grid.clone()));
        // The Settings tab, opened over the window and in front.
        let diag = Rc::new(FakeDiag::new(crate::settings_scene::LOG_DIR, false));
        let mut sdeps = crate::settings_scene::deps(&prefs, post, {
            let c = clock.clone();
            Rc::new(move || c.get())
        });
        sdeps.diag = diag.clone();
        let settings = SettingsCtl::new(&h.comp, sdeps);
        {
            let mut c = ctl.borrow_mut();
            c.set_settings(settings.clone());
            c.model_mut().open_settings();
            c.refresh();
        }
        let f = Fx {
            h,
            ctl,
            q,
            prefs,
            look_due,
            grid,
            scale,
            diag,
            _settings: settings,
            _dir: dir,
        };
        f.pump();
        assert_eq!(f.h.comp.get_front_kind(), SETTINGS, "Settings is in front");
        f
    }

    /// One turn of the event loop: every queued message, then the look's coalesced apply.
    fn pump(&self) {
        for _ in 0..50 {
            let m = self.q.lock().unwrap().pop_front();
            if let Some(m) = m {
                self.ctl.borrow_mut().on_msg(m);
                continue;
            }
            if self.look_due.swap(false, Ordering::SeqCst) {
                LookCtl::with_current(|l| l.apply_now());
                continue;
            }
            break;
        }
        self.h.advance(0);
    }

    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    fn page(&self, id: &str) {
        self.b().invoke_page_chosen(id.into());
        self.pump();
    }

    fn choose(&self, row: &str, option: &str) {
        self.b().invoke_row_choose(row.into(), option.into());
        self.pump();
    }

    fn switch(&self, row: &str, on: bool) {
        self.b().invoke_row_switch(row.into(), on);
        self.pump();
    }

    /// Settings is still the tab in front: what changed, changed under it.
    fn still_open(&self, what: &str) {
        assert_eq!(
            self.h.comp.get_front_kind(),
            SETTINGS,
            "scale {}: Settings stays in front after {what}",
            self.scale
        );
    }

    fn theme(&self) -> Theme<'_> {
        Theme::get(&self.h.comp)
    }
}

/// The root of the owner's bug: the tab view read the tab position only on a refresh, and nothing
/// refreshed it while Settings stayed open.
#[test]
fn the_tab_position_moves_the_tabs_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("appearance");
        assert!(!f.h.comp.get_layout_vertical(), "scale {s}: along the top");
        f.choose("tab-layout", "vertical");
        assert_eq!(tab_layout(&f.prefs), TabLayout::Vertical);
        assert!(
            f.h.comp.get_layout_vertical(),
            "scale {s}: the tabs went down the side with Settings open"
        );
        assert!(f.h.comp.get_sidebar_width() > 0.0, "scale {s}: a sidebar");
        f.still_open("vertical");
        f.choose("tab-layout", "horizontal");
        assert!(
            !f.h.comp.get_layout_vertical(),
            "scale {s}: back along the top with Settings open"
        );
        f.still_open("horizontal");
    }
}

/// The keys the window reads when a key is pressed (agent hotkeys, keep agent sessions open).
/// The chord context reads the store on every key, so the value is live by itself; what the row
/// needs is for the main window to hear the change (its refresh re-pushes whatever the view shows
/// from it). No label the view pushes depends on these two rows (only Next tab and Sidebar have an
/// agent variant, and they are not shown in the window), so the proof is the message the window's
/// own listener receives, per key of each row, with Settings in front.
#[test]
fn the_key_rows_hold_for_the_next_key_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("terminal");
        for id in ["agent-keys", "agent-guard"] {
            let row = flare_settings::rows::all_rows()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("row {id}"));
            assert!(!row.keys.is_empty(), "{id} stores a key");
            for key in row.keys {
                f.pump();
                f.q.lock().unwrap().clear();
                let now = f.prefs.get(key).unwrap_or_default();
                f.prefs.set(key, if now == "1" { "0" } else { "1" });
                assert!(
                    f.q.lock()
                        .unwrap()
                        .iter()
                        .any(|m| matches!(m, UiMsg::PrefsChanged)),
                    "scale {s}: the main window hears {key} ({id}) with Settings open"
                );
                f.pump();
            }
        }
        // And through the real controls the window's key context follows.
        let before = f.ctl.borrow().key_context();
        f.switch("agent-keys", !before.agent_keys);
        f.switch("agent-guard", !before.agent_guard);
        let after = f.ctl.borrow().key_context();
        assert_eq!(after.agent_keys, !before.agent_keys, "scale {s}");
        assert_eq!(after.agent_guard, !before.agent_guard, "scale {s}");
        f.still_open("the key rows");
    }
}
/// The look rows: panel edges, a window colour and a theme are worn by the window at once.
#[test]
fn the_look_rows_change_the_window_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("appearance");
        let line = f.theme().get_line();
        f.choose("window-edges", "solid");
        let solid = f.theme().get_line();
        assert_ne!(solid, line, "scale {s}: solid edges are worn at once");
        f.choose("window-edges", "none");
        assert_eq!(
            f.theme().get_line().alpha(),
            0,
            "scale {s}: no edges at once"
        );
        f.still_open("the edges");

        let accent = f.theme().get_accent();
        f.b()
            .invoke_colour_edited("window-accent".into(), "#ff0000".into());
        f.b().invoke_colour_committed("window-accent".into());
        f.pump();
        let mut want = String::new();
        LookCtl::with_current(|l| {
            want = l
                .current_look()
                .chrome
                .vars
                .get("--p-accent")
                .cloned()
                .unwrap_or_default();
        });
        let now = f.theme().get_accent();
        assert_ne!(now, accent, "scale {s}: the accent changed at once");
        assert_eq!(
            now,
            crate::chrome_apply::colour(&want),
            "scale {s}: the window wears the picked accent ({want})"
        );
        assert!(now.red() > 200 && now.green() < 40, "scale {s}: red");
        f.still_open("the accent");

        let ground = f.theme().get_ground();
        f.b().invoke_wall_pick("paper".into());
        f.pump();
        // A dirty theme asks first; Discard lands the pick.
        if f.b().get_ask_open() {
            f.b().invoke_ask_answered(1);
            f.pump();
        }
        assert_eq!(LookStore::new(f.prefs.clone()).theme_id(), "paper");
        assert_ne!(
            f.theme().get_ground(),
            ground,
            "scale {s}: the new theme's ground is worn at once"
        );
        f.still_open("the theme");
    }
}

/// The terminal font rows: the grid's cells change size at once.
#[test]
fn the_terminal_font_rows_reach_the_grid_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("terminal");
        let cell = f.grid.borrow().metrics().cell_h;
        f.choose("term-font", "150");
        let bigger = f.grid.borrow().metrics().cell_h;
        assert!(
            bigger > cell,
            "scale {s}: the cells grew at once ({cell} to {bigger})"
        );
        f.still_open("the text size");
    }
}

/// Command help: switched off, its button leaves the window at once.
#[test]
fn command_help_goes_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("terminal");
        assert!(f.h.comp.get_help_visible(), "scale {s}");
        f.switch("help-enabled", false);
        assert!(!f.h.comp.get_help_visible(), "scale {s}: gone at once");
        f.still_open("command help");
    }
}

/// The Diagnostics rows: detailed logging switches at once, and Mark a problem stamps the log now.
#[test]
fn the_diagnostics_rows_act_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::new(s);
        f.page("diagnostics");
        assert!(!f.diag.verbose(), "scale {s}");
        f.switch("diag-verbose", true);
        assert!(
            f.diag.verbose(),
            "scale {s}: detailed logging is on at once"
        );
        f.b().invoke_row_button("diag-mark".into(), "mark".into());
        f.pump();
        assert_eq!(f.diag.marks(), 1, "scale {s}: the moment is stamped");
        f.still_open("the diagnostics rows");
    }
}

/// The sweep's guard: every key a row stores and the look, the help panel and the dictation
/// controller do not own reaches the tab view's own listener, so a row added later is live too.
/// The owned keys are checked from the other side: the look keys are heard as a look change, and
/// none of them is also taken by the generic refresh.
#[test]
fn every_other_row_key_reaches_the_main_window() {
    let f = Fx::new(1.0);
    let owned: BTreeSet<&str> = flare_settings::look::LOOK_KEYS
        .iter()
        .chain(flare_settings::agent_prefs::AGENT_LOOK_KEYS.iter())
        .chain(flare_settings::help_prefs::HELP_KEYS.iter())
        .chain(flare_dictation::prefs::KEYS.iter())
        .copied()
        .collect();
    let keys: BTreeSet<&str> = flare_settings::rows::all_rows()
        .flat_map(|r| r.keys.iter().copied())
        .filter(|k| !owned.contains(k))
        .collect();
    assert!(keys.contains("flare.window.tabLayout"));
    let heard = |f: &Fx, want: fn(&UiMsg) -> bool| f.q.lock().unwrap().iter().any(want);
    for key in keys {
        f.pump();
        f.q.lock().unwrap().clear();
        f.prefs.set(key, "x");
        assert!(
            heard(&f, |m| matches!(m, UiMsg::PrefsChanged)),
            "{key}: the main window hears it"
        );
    }
    // The look keys are the window's own through the look message, not the generic refresh.
    for key in flare_settings::look::LOOK_KEYS
        .iter()
        .chain(flare_settings::agent_prefs::AGENT_LOOK_KEYS.iter())
    {
        f.pump();
        f.q.lock().unwrap().clear();
        f.prefs.set(key, "x");
        assert!(
            heard(&f, |m| matches!(m, UiMsg::LookChanged)),
            "{key}: the window hears it as a look change"
        );
        assert!(
            !heard(&f, |m| matches!(m, UiMsg::PrefsChanged)),
            "{key}: and the generic refresh does not also take it"
        );
    }
    // The help and dictation groups are named, so a key moved between owners is a deliberate edit.
    assert_eq!(flare_settings::help_prefs::HELP_KEYS.len(), 3);
    assert_eq!(flare_dictation::prefs::KEYS.len(), 9);
}
