//! Interaction tests through the real window (Task 31): one `Harness<AppWindow>` plus an `AppCtl`
//! over fakes per test (no real shell, no OS window), driven the way the winit filter drives it:
//! pointer events go through Slint, keys go through `AppCtl::on_key` first and reach a Slint
//! control only when it answers `Slint`. Pointer positions come from the rendered layout (the
//! divider columns of the strip row, the sidebar's row grid), never from magic numbers.
//! The test names carry the e2e scenario of `docs/parity.md` they port.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_agent::kinds::AgentKind;
use flare_settings::kv::PrefStore;
use flare_settings::new_tab::set_new_tab_folder;
use flare_settings::recent_roots::{forget_root, pinned_roots, recent_roots, remember_root};
use flare_settings::sidebar::{SidebarMode, sidebar_mode};
use flare_settings::tab_layout::{TabLayout, set_tab_layout};
use flare_settings::tab_switch::{TabSwitch, set_tab_switch};
use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, tabs_json};
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_term::keys::KeyEvent;
use flare_win::single_instance::Handoff;
use flare_win::window_state::WindowStateStore;
use slint::platform::PointerEventButton;
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer};

use crate::appctl::{AppCtl, Deps, KeyOutcome, Post, UiMsg};
use crate::startup::{ask_view, tab_ask};
use crate::tab_host::TabHost;
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, TabRow};

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

/// `Theme.ground`, the strip's divider over it, the test grid image and the loading ring's ink.
const GROUND: [u8; 4] = [23, 23, 23, 255];
const DIVIDER: [u8; 4] = [39, 39, 39, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const RING: [u8; 4] = [0xfe, 0x8f, 0x34, 255];

const BAR_H: f32 = 36.0;
#[cfg(windows)]
const HOME: &str = "C:\\home";
/// A POSIX home on macOS, so the folder rows read it as `~` (`flare_tabs::sidebar_rows::folder_line`).
/// A Mac home lives under `/Users`: `/home` is the `auto_home` automount point, which a restore refuses.
#[cfg(not(windows))]
const HOME: &str = "/Users/me";

#[derive(Default)]
struct Log {
    picks: Vec<Option<PathBuf>>,
    raised: usize,
    quit: usize,
    hidden: usize,
    close_clicks: u32,
    minimize_clicks: u32,
    maximize_clicks: u32,
    sources: Vec<Option<String>>,
}

#[derive(Default)]
struct Opts {
    scale: Option<f32>,
    size: Option<(u32, u32)>,
    saved: Vec<(String, Option<AgentKind>)>,
    active: usize,
    real_fs: bool,
    pick_answer: Option<PathBuf>,
}

impl Opts {
    fn saved(mut self, names: &[&str]) -> Self {
        self.saved = names.iter().map(|n| ((*n).to_string(), None)).collect();
        self
    }
    fn scale(mut self, s: f32) -> Self {
        self.scale = Some(s);
        self
    }
    fn size(mut self, w: u32, h: u32) -> Self {
        self.size = Some((w, h));
        self
    }
}

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Queue,
    sp: Arc<FakeSpawner>,
    dir: tempfile::TempDir,
    clock: Rc<Cell<u64>>,
    log: Rc<RefCell<Log>>,
    prefs: PrefStore,
    scale: f32,
    css: (u32, u32),
}

fn grid_image() -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(8, 8);
    for p in buf.make_mut_slice() {
        *p = Rgba8Pixel {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
    }
    Image::from_rgba8(buf)
}

fn names(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("C:\\w\\proj{i:02}")).collect()
}

impl Fx {
    fn new(opts: Opts) -> Fx {
        Self::with_prefs(opts, |_| {})
    }

    fn with_prefs(opts: Opts, prep: impl FnOnce(&PrefStore)) -> Fx {
        let scale = opts.scale.unwrap_or(1.0);
        let css = opts.size.unwrap_or((1000, 640));
        let dir = tempfile::tempdir().unwrap();
        if !opts.saved.is_empty() {
            let saved = SavedTabs {
                tabs: opts
                    .saved
                    .iter()
                    .map(|(cwd, agent)| SavedTab {
                        cwd: cwd.clone(),
                        agent: *agent,
                    })
                    .collect(),
                active: opts.active,
            };
            std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test code only, never the app
        }
        let prefs = PrefStore::memory();
        prep(&prefs);
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
        let host = TabHost::new(sp.clone(), FakeShells::new(), prefs.clone(), waker, None);
        let clock = Rc::new(Cell::new(0u64));
        let log: Rc<RefCell<Log>> = Rc::default();
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = if opts.real_fs {
            Arc::new(|p| Path::new(p).is_dir())
        } else {
            Arc::new(|p| !p.contains("gone"))
        };
        let answer = opts.pick_answer.clone();
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
            is_dir,
            home: HOME.to_string(),
            clock: {
                let c = clock.clone();
                Rc::new(move || c.get())
            },
            post,
            pick_folder: {
                let log = log.clone();
                Rc::new(move |default, done| {
                    log.borrow_mut().picks.push(default);
                    done(answer.clone());
                })
            },
            clipboard: Rc::new(|_| {}),
            open_url: Rc::new(|_| {}),
            set_grid_source: {
                let log = log.clone();
                Rc::new(move |h| {
                    log.borrow_mut().sources.push(h.map(|h| h.id().to_string()));
                })
            },
            raise: {
                let log = log.clone();
                Rc::new(move || log.borrow_mut().raised += 1)
            },
            quit: {
                let log = log.clone();
                Rc::new(move || log.borrow_mut().quit += 1)
            },
            hide: {
                let log = log.clone();
                Rc::new(move || log.borrow_mut().hidden += 1)
            },
            starter_folders: Rc::new(|_| {}),
            launch_folders: Vec::new(),
            e2e: true,
            term: crate::appctl::TermPorts::inert(),
        };
        let h = Harness::new(scale, css.0, css.1, || AppWindow::new().unwrap());
        h.comp.set_grid_image(grid_image());
        let ctl = AppCtl::new(&h.comp, deps);
        // Pinned: the runner's Windows "show animations" switch must not decide the clock or the marks (the help panel and chrome motion still read the OS).
        ctl.borrow_mut().set_animations_source(Rc::new(|| true));
        // The window glue (Task 30b) in miniature: the grid size reaches the controller, cells
        // are 8 x 16 device pixels, and the three window buttons are counted.
        {
            let weak = Rc::downgrade(&ctl);
            h.comp.on_grid_resized(move |w, hgt, _scale| {
                if let Some(rc) = weak.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.on_grid_dims((w / 8).max(2) as usize, (hgt / 16).max(1) as usize);
                }
            });
            let l = log.clone();
            h.comp
                .on_close_clicked(move || l.borrow_mut().close_clicks += 1);
            let l = log.clone();
            h.comp
                .on_minimize_clicked(move || l.borrow_mut().minimize_clicks += 1);
            let l = log.clone();
            h.comp
                .on_maximize_clicked(move || l.borrow_mut().maximize_clicks += 1);
        }
        h.advance(0);
        ctl.borrow_mut().refresh();
        Fx {
            h,
            ctl,
            q,
            sp,
            dir,
            clock,
            log,
            prefs,
            scale,
            css,
        }
    }

    // ---- plumbing -------------------------------------------------------------------------

    fn pump(&self) {
        loop {
            let m = self.q.lock().unwrap().pop_front();
            let Some(m) = m else { break };
            self.ctl.borrow_mut().on_msg(m);
        }
        self.h.advance(0);
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

    fn restore(&self) {
        AppCtl::start_restore(&self.ctl);
        self.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
        self.h.advance(0);
        self.ctl.borrow_mut().refresh();
        self.h.advance(0);
    }

    /// The window clock and the Slint animation clock move together; the controller ticks.
    fn step(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
        self.h.advance(ms);
        self.ctl.borrow_mut().tick();
        self.pump();
    }

    fn rows(&self) -> usize {
        self.h.comp.get_tab_rows().row_count()
    }

    fn row(&self, i: usize) -> TabRow {
        self.h.comp.get_tab_rows().row_data(i).unwrap()
    }

    fn all_rows(&self) -> Vec<TabRow> {
        (0..self.rows()).map(|i| self.row(i)).collect()
    }

    fn cwds(&self) -> Vec<String> {
        self.ctl
            .borrow()
            .model()
            .state
            .tabs
            .iter()
            .map(|t| t.cwd.clone())
            .collect()
    }

    fn front_cwd(&self) -> String {
        self.ctl.borrow().model().active().unwrap().cwd.clone()
    }

    fn key(&self, ev: KeyEvent) -> KeyOutcome {
        self.ctl.borrow_mut().on_key(&ev)
    }

    fn key_up(&self, ev: KeyEvent) {
        self.ctl.borrow_mut().on_key_up(&ev);
    }

    /// A key the app does not take: the focused Slint control gets it (the filter's `Slint`).
    fn type_text(&self, text: &str) {
        assert!(
            self.h.comp.get_chrome_has_focus(),
            "a Slint control should hold the keys"
        );
        self.h.key_text(text);
        self.h.advance(0);
    }

    fn made_with(&self, cwd: &str) -> Vec<Arc<flare_term::fake::FakeState>> {
        self.sp
            .made()
            .into_iter()
            .filter(|m| m.spec().cwd == cwd)
            .collect()
    }

    fn wait_made(&self, cwd: &str) {
        self.pump_until("a spawn", |f| !f.made_with(cwd).is_empty());
    }

    /// No key reached any shell. The emulator's own answers to a program's queries (the cursor
    /// report and the device attributes a starting shell asks for) are not keys and are ignored.
    fn nothing_was_typed(&self) -> bool {
        self.sp.made().iter().all(|m| {
            let mut w = String::from_utf8_lossy(&m.written()).into_owned();
            for reply in ["\x1b[1;1R", "\x1b[?1;2c"] {
                w = w.replace(reply, "");
            }
            w.is_empty()
        })
    }

    /// Opens a tab the way a user does, with Ctrl+T, in the user's folder.
    fn ctrl_t(&self) {
        assert_eq!(self.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
        self.h.advance(0);
    }

    /// Opens a folder tab through the + menu's recent list (a real click path) when `path` is
    /// among the recents, else through a hand-off.
    fn open_via_handoff(&self, rel: &str) {
        self.ctl.borrow_mut().handoff(Handoff {
            cwd: self.dir.path().to_string_lossy().into_owned(),
            args: vec!["flare.exe".to_string(), rel.to_string()],
        });
    }

    // ---- layout, from the rendered window ---------------------------------------------------

    fn hair_phys(&self) -> u32 {
        self.scale.floor().max(1.0) as u32
    }

    fn strip_left_phys(&self) -> u32 {
        (42.0 * self.scale).round() as u32 + self.hair_phys()
    }

    fn park_pointer(&self) {
        self.h
            .move_to(self.css.0 as f32 - 5.0, self.css.1 as f32 - 5.0);
        self.h.advance(0);
    }

    /// The end (exclusive, device px) of every full-height divider in the strip row: the first
    /// is the mark cell's line, so tab `i` spans `edges[i] .. edges[i + 1]`.
    fn divider_edges(&self, f: &Frame) -> Vec<u32> {
        let y = self.h.phys(6.0);
        let hair = self.hair_phys();
        let is_div = |p: [u8; 4]| Frame::near(p, DIVIDER, 2);
        let mut out = Vec::new();
        let mut x = 0;
        while x < f.w {
            if is_div(f.px(x, y)) {
                let run = f.run_len_h(y, x, is_div);
                assert!(run >= hair, "a divider run of {run} device px at x {x}");
                out.push(x + run);
                x += run;
            } else {
                x += 1;
            }
        }
        out
    }

    /// The css centre of tab `i`, found by scanning the strip row for divider columns.
    fn tab_center(&self, i: usize) -> (f32, f32) {
        self.park_pointer();
        let f = self.h.render();
        let e = self.divider_edges(&f);
        assert!(e.len() > i + 1, "no divider after tab {i}: {e:?}");
        let mid = (e[i] + e[i + 1]) as f32 / 2.0 / self.scale;
        (mid, BAR_H / 2.0)
    }

    /// The css centre of the + button.
    fn plus_center(&self) -> (f32, f32) {
        let g = self.h.comp.get_strip_geom();
        let x = self.strip_left_phys() as f32 + g.plus_x as f32 + g.plus_w as f32 / 2.0;
        (x / self.scale, BAR_H / 2.0)
    }

    /// The css centre of the +N button.
    fn more_center(&self) -> (f32, f32) {
        let g = self.h.comp.get_strip_geom();
        let x = self.strip_left_phys() as f32 + g.more_x as f32 + g.more_w as f32 / 2.0;
        (x / self.scale, BAR_H / 2.0)
    }

    /// The sidebar's row `i`: 36 bar + 50 find box, then 46 tall cards 4 apart.
    fn side_row_center(&self, i: usize) -> (f32, f32) {
        (110.0, BAR_H + 50.0 + 50.0 * i as f32 + 23.0)
    }

    fn start_new_center(&self, places: usize) -> (f32, f32) {
        let s = self.scale;
        let body = self.css.1 as f32 - BAR_H;
        let nav = if places > 0 {
            112.0 + 38.0 * places as f32
        } else {
            93.0
        };
        let block = 84.0 + 16.0 + 28.0 + 40.0 + nav;
        let top = ((body - 36.0 - block) / 2.0 * s).round() / s;
        (self.css.0 as f32 / 2.0, BAR_H + top + 168.0 + 18.0)
    }

    fn start_place_center(&self, places: usize, i: usize) -> (f32, f32) {
        let (x, y) = self.start_new_center(places);
        let top = y - 18.0;
        let place = ((57.0 + 38.0 * i as f32) * self.scale).round() / self.scale;
        (x, top + place + 18.0)
    }

    /// The window button `n` counted from the right (0 close, 1 maximise, 2 minimise, 3 settings).
    fn button_center(&self, n: u32) -> (f32, f32) {
        (self.css.0 as f32 - 22.0 - 36.0 * n as f32, BAR_H / 2.0)
    }
}

fn ctrl(key: &str, code: &str) -> KeyEvent {
    KeyEvent::new(key, code).ctrl()
}

fn is_divider(p: [u8; 4]) -> bool {
    Frame::near(p, DIVIDER, 2)
}

fn is_ground(p: [u8; 4]) -> bool {
    Frame::near(p, GROUND, 2)
}

// ---------------------------------------------------------------------------------------------

#[test]
fn new_terminal_from_the_start_screen_opens_one_tab_in_the_home_folder() {
    let f = Fx::new(Opts::default());
    f.restore();
    assert_eq!(
        f.h.comp.get_front_kind(),
        2,
        "no tabs.json: the start screen"
    );
    f.h.advance(500);
    let (x, y) = f.start_new_center(0);
    f.h.click(x, y);
    f.pump_until("the tab", |f| f.rows() == 1);
    assert_eq!(f.cwds(), [HOME]);
    f.wait_made(HOME);
    // The tooltip: a second of rest, then the full path.
    let (tx, ty) = f.tab_center(0);
    f.h.move_to(tx, ty);
    f.h.advance(0);
    f.clock.set(1001);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_tip_visible());
    assert_eq!(f.h.comp.get_tip_text().as_str(), HOME);
    // Ctrl+T opens a second.
    f.ctrl_t();
    f.pump_until("the second tab", |f| f.rows() == 2);
    assert!(f.nothing_was_typed(), "no tab chord reached the pty");
}

#[test]
fn the_start_screen_footer_says_version_in_words() {
    // Visual review (Task 33): the footer read "v0.1.0". PT and the plan: "Version 0.1.0".
    let f = Fx::new(Opts::default());
    f.restore();
    assert_eq!(f.h.comp.get_front_kind(), 2);
    assert_eq!(
        f.h.comp.get_version_text().as_str(),
        format!("Version {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn every_tab_is_one_width_and_picking_moves_no_tab() {
    for s in [1.0_f32, 1.5, 2.25] {
        let f = Fx::new(
            Opts::default()
                .scale(s)
                .size(800, 600)
                .saved(&["C:\\a", "C:\\b", "C:\\c", "C:\\d", "C:\\e"]),
        );
        f.restore();
        let rows = f.all_rows();
        assert_eq!(rows.len(), 5, "scale {s}");
        let w0 = rows[0].w;
        for r in &rows {
            assert!((r.w - w0).abs() <= 1, "scale {s}: widths {} vs {w0}", r.w);
            let css = r.w as f32 / s;
            assert!(
                (84.0 - 1.0 / s..=150.0 + 1.0 / s).contains(&css),
                "scale {s}: a tab is {css} css px wide"
            );
        }
        let before: Vec<(i32, i32)> = rows.iter().map(|r| (r.x, r.w)).collect();
        let (x, y) = f.tab_center(2);
        f.h.click(x, y);
        f.h.advance(0);
        assert_eq!(f.front_cwd(), "C:\\c", "scale {s}");
        let after: Vec<(i32, i32)> = f.all_rows().iter().map(|r| (r.x, r.w)).collect();
        assert_eq!(before, after, "scale {s}: picking moved a tab");
    }
}

#[test]
fn a_faint_divider_at_every_right_edge_and_the_icons_are_18px() {
    for s in [1.0_f32, 1.5, 2.25] {
        let f = Fx::new(
            Opts::default()
                .scale(s)
                .size(800, 600)
                .saved(&["C:\\a", "C:\\b", "C:\\c"]),
        );
        f.restore();
        f.park_pointer();
        let fr = f.h.render();
        let rows = f.all_rows();
        let edges = f.divider_edges(&fr);
        let left = f.strip_left_phys();
        assert_eq!(
            edges.len(),
            4,
            "scale {s}: mark line plus three tabs: {edges:?}"
        );
        assert_eq!(
            edges[0], left,
            "scale {s}: the strip starts after the mark's line"
        );
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(
                edges[i + 1],
                left + (r.x + r.w) as u32,
                "scale {s}: tab {i}'s divider is at its right edge"
            );
            // No full border: the pixel row above the tab (its top edge) is ground or the rule.
            let mid = left + r.x as u32 + (r.w as u32) / 2;
            let top = fr.px(mid, 0);
            assert!(
                is_ground(top) || !is_divider(top),
                "scale {s}: tab {i} has a top border"
            );
            // The icon: 18 css px box at 10 css px from the left edge.
            let x0 = left + r.x as u32 + f.h.phys(6.0);
            let x1 = left + r.x as u32 + f.h.phys(32.0);
            let bb = fr
                .bbox_where(x0, f.h.phys(4.0), x1, f.h.phys(32.0), |p| !is_ground(p))
                .unwrap_or_else(|| panic!("scale {s}: tab {i} draws no icon"));
            let w = (bb.2 - bb.0 + 1) as f32 / s;
            let hgt = (bb.3 - bb.1 + 1) as f32 / s;
            assert!((8.0..=19.0).contains(&w), "scale {s}: icon {w} css px wide");
            assert!(
                (8.0..=19.0).contains(&hgt),
                "scale {s}: icon {hgt} css px tall"
            );
            let cy = (bb.1 + bb.3) as f32 / 2.0 / s;
            assert!((cy - 17.5).abs() <= 2.0, "scale {s}: icon centred at {cy}");
            // The name is centred between its icon and its end.
            let t0 = left + r.x as u32 + f.h.phys(34.0);
            let t1 = left + (r.x + r.w) as u32 - f.h.phys(32.0);
            let tb = fr
                .bbox_where(t0, f.h.phys(8.0), t1, f.h.phys(28.0), |p| !is_ground(p))
                .unwrap_or_else(|| panic!("scale {s}: tab {i} draws no name"));
            let centre = (tb.0 + tb.2) as f32 / 2.0;
            let want = (t0 + t1) as f32 / 2.0;
            assert!(
                (centre - want).abs() <= 3.0 * s,
                "scale {s}: name centre {centre} vs {want}"
            );
        }
        // The + comes right after the last tab.
        let g = f.h.comp.get_strip_geom();
        let last = rows.last().unwrap();
        assert!(
            g.plus_x >= last.x + last.w,
            "scale {s}: + after the last tab"
        );
        let px0 = left + g.plus_x as u32;
        let drawn = fr.count_where(
            px0,
            f.h.phys(6.0),
            px0 + g.plus_w as u32,
            f.h.phys(30.0),
            |p| !is_ground(p),
        );
        assert!(drawn > 0, "scale {s}: the + is drawn");
    }
}

#[test]
fn ctrl_tab_in_order_and_most_recent() {
    // Stored `order`: the next tab, wrapping.
    let f = Fx::with_prefs(Opts::default().saved(&["C:\\a", "C:\\b", "C:\\c"]), |p| {
        set_tab_switch(p, TabSwitch::Order)
    });
    f.restore();
    assert_eq!(f.front_cwd(), "C:\\a");
    f.key(KeyEvent::new("Tab", "Tab").ctrl());
    assert_eq!(f.front_cwd(), "C:\\b");
    f.key(KeyEvent::new("Tab", "Tab").ctrl());
    f.key(KeyEvent::new("Tab", "Tab").ctrl());
    assert_eq!(f.front_cwd(), "C:\\a", "the walk wraps");

    // Stored `recent`: A, B, C used; Ctrl+Tab goes to the previous-used; held, further back.
    let g = Fx::with_prefs(Opts::default().saved(&["C:\\a", "C:\\b", "C:\\c"]), |p| {
        set_tab_switch(p, TabSwitch::Recent)
    });
    g.restore();
    for i in 0..3 {
        let (x, y) = g.tab_center(i);
        g.h.click(x, y);
        g.h.advance(0);
    }
    assert_eq!(g.front_cwd(), "C:\\c");
    g.key(KeyEvent::new("Tab", "Tab").ctrl());
    assert_eq!(g.front_cwd(), "C:\\b", "the previous-used");
    g.key(KeyEvent::new("Tab", "Tab").ctrl());
    assert_eq!(g.front_cwd(), "C:\\a", "held: further back");
    g.key_up(KeyEvent::new("Control", "ControlLeft").released());
    g.key(KeyEvent::new("Tab", "Tab").ctrl());
    assert_eq!(
        g.front_cwd(),
        "C:\\c",
        "the committed walk made A the latest, so the one before it is C"
    );
    g.key_up(KeyEvent::new("Control", "ControlLeft").released());
    // Ctrl+W closes the front.
    assert_eq!(g.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert_eq!(g.cwds(), ["C:\\a", "C:\\b"]);
    assert!(g.nothing_was_typed());
}

#[test]
fn the_cog_and_ctrl_comma_toggle_settings() {
    let f = Fx::new(Opts::default().saved(&["C:\\a", "C:\\b"]));
    f.restore();
    let (cx, cy) = f.button_center(3);
    f.h.click(cx, cy);
    f.h.advance(0);
    assert_eq!(f.h.comp.get_front_kind(), 1, "the Settings page");
    assert_eq!(f.rows(), 3);
    assert!(f.all_rows().iter().any(|r| r.settings && r.active));
    f.h.click(cx, cy);
    f.h.advance(0);
    assert_eq!(f.h.comp.get_front_kind(), 0);
    assert_eq!(f.rows(), 2);
    assert_eq!(f.front_cwd(), "C:\\a", "the previous tab is back in front");
    // Ctrl+, does the same.
    assert_eq!(f.key(ctrl(",", "Comma")), KeyOutcome::Consumed);
    assert_eq!(f.h.comp.get_front_kind(), 1);
    assert_eq!(f.key(ctrl(",", "Comma")), KeyOutcome::Consumed);
    assert_eq!(f.h.comp.get_front_kind(), 0);
    assert_eq!(f.rows(), 2);
}

#[test]
fn the_cog_has_a_tip_with_its_key_and_it_stays_inside_the_window() {
    let f = Fx::new(Opts::default().saved(&["C:\\a"]));
    f.restore();
    let (cx, cy) = f.button_center(3);
    f.h.move_to(cx, cy);
    f.h.advance(0);
    f.clock.set(f.clock.get() + 1_001);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_tip_visible(), "hovering the cog shows its tip");
    assert_eq!(f.h.comp.get_tip_text().as_str(), "Settings (Ctrl+,)");
    assert!(
        f.h.comp.get_tip_x() + f.h.comp.get_tip_w() <= 1000.0,
        "the tip stays inside the window"
    );
}

#[test]
fn closing_the_last_tab_lands_on_the_start_screen() {
    let f = Fx::with_prefs(Opts::default().saved(&["C:\\p1"]), |p| {
        remember_root(p, "C:\\p1");
        remember_root(p, "C:\\p2");
    });
    f.restore();
    assert_eq!(f.rows(), 1);
    let (x, y) = f.tab_center(0);
    f.h.middle_click(x, y);
    f.h.advance(500);
    assert_eq!(f.rows(), 0);
    assert_eq!(f.h.comp.get_front_kind(), 2, "the start screen");
    assert_eq!(f.log.borrow().quit, 0, "the window stays");
    assert_eq!(f.h.comp.get_places().row_count(), 2);
    let want = slint::Model::row_data(&f.h.comp.get_places(), 0)
        .unwrap()
        .path
        .to_string();
    let (px, py) = f.start_place_center(2, 0);
    f.h.click(px, py);
    f.pump_until("the tab", |f| f.rows() == 1);
    assert_eq!(f.cwds(), std::slice::from_ref(&want));
    f.wait_made(&want);
    assert_eq!(f.log.borrow().quit, 0);
}

#[test]
fn dragging_the_first_tab_moves_it_and_not_the_window() {
    let f = Fx::new(Opts::default().saved(&["C:\\a", "C:\\b", "C:\\c"]));
    f.restore();
    assert_eq!(f.cwds(), ["C:\\a", "C:\\b", "C:\\c"]);
    let (x, y) = f.tab_center(0);
    f.h.move_to(x, y);
    f.h.press(x, y, PointerEventButton::Left);
    // The press reached the strip (not a window drag): moving carries the tab.
    for i in 1..=6 {
        f.h.move_to(x + 20.0 * i as f32, y);
        f.h.advance(0);
        if i == 3 {
            assert!(f.h.comp.get_carrying(), "the first tab is carried");
        }
    }
    f.h.release(x + 120.0, y, PointerEventButton::Left);
    f.h.advance(0);
    assert!(!f.h.comp.get_carrying());
    assert_ne!(f.cwds(), ["C:\\a", "C:\\b", "C:\\c"], "the order changed");
    assert_eq!(f.cwds()[0], "C:\\b");
    // The strip stays a window handle only while no file drag is in flight.
    assert!(!f.h.comp.get_drag_in_flight());
}

#[test]
fn sixteen_tabs_icons_plus_n_and_the_list() {
    let all = names(16);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let f = Fx::new(Opts::default().size(700, 600).saved(&refs));
    f.restore();
    let rows = f.all_rows();
    assert_eq!(rows.len(), 16);
    let hidden = rows.iter().filter(|r| r.mode == 2).count() as i32;
    let drawn = rows.iter().filter(|r| r.mode != 2).count() as i32;
    let g = f.h.comp.get_strip_geom();
    assert!(hidden > 0, "700 px cannot hold sixteen tabs");
    assert_eq!(g.more_count, hidden, "the +N count is the hidden tabs");
    assert_eq!(drawn + g.more_count, 16);
    let named: Vec<&TabRow> = rows.iter().filter(|r| r.mode == 0).collect();
    assert_eq!(named.len(), 1, "only the front keeps its name");
    assert!(named[0].active);
    assert!(rows.iter().any(|r| r.mode == 1), "the others are icons");

    // The list: opens from the +N button, narrows by typing, Enter fronts the first kept.
    let (mx, my) = f.more_center();
    f.h.click(mx, my);
    f.h.advance(0);
    assert!(f.h.comp.get_overflow_open());
    assert_eq!(f.h.comp.get_overflow_rows().row_count() as i32, hidden);
    f.type_text("proj16");
    assert_eq!(f.h.comp.get_overflow_rows().row_count(), 1);
    f.type_text("\n");
    assert!(!f.h.comp.get_overflow_open());
    assert_eq!(f.front_cwd(), "C:\\w\\proj16");
    // Ctrl+Shift+K lists every tab.
    assert_eq!(
        f.key(KeyEvent::new("K", "KeyK").ctrl().shift()),
        KeyOutcome::Consumed
    );
    assert!(f.h.comp.get_overflow_open());
    assert_eq!(f.h.comp.get_overflow_rows().row_count(), 16);
}

/// Port of the manyTabs scenario: Escape closes the list. It must work while the field has the
/// keys, which is the only place the keys are once the list opens.
#[test]
fn escape_closes_the_plus_n_list_from_its_field() {
    let all = names(16);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let f = Fx::new(Opts::default().size(700, 600).saved(&refs));
    f.restore();
    let (mx, my) = f.more_center();
    f.h.click(mx, my);
    f.h.advance(0);
    assert!(f.h.comp.get_overflow_open());
    f.type_text("\u{1b}");
    assert!(!f.h.comp.get_overflow_open(), "Escape closes the list");
}

fn vertical(p: &PrefStore) {
    set_tab_layout(p, TabLayout::Vertical);
}

/// CSS `cubic-bezier(0.16, 1, 0.3, 1)` at time fraction `t`, solved apart from the app's own code.
fn css_ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let at = |a: f64, b: f64, s: f64| {
        3.0 * a * (1.0 - s).powi(2) * s + 3.0 * b * (1.0 - s) * s * s + s.powi(3)
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if at(0.16, 0.3, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(1.0, 1.0, (lo + hi) / 2.0)
}

/// The pin/unpin slide starts at its first frame, as a CSS transition does, not at the key: time
/// that passes between the key and the first frame is not skipped (audit V3, the first frame jumped
/// well into the curve). `sliding` is true at once, and the slide follows the declared curve.
#[test]
fn the_sidebar_slide_starts_at_its_first_frame_and_follows_the_curve() {
    for scale in [1.0, 1.5, 2.25, 3.0] {
        let f = Fx::with_prefs(
            Opts::default()
                .scale(scale)
                .saved(&["C:\\w\\api", "C:\\w\\web"]),
            vertical,
        );
        f.restore();
        for _ in 0..30 {
            f.step(16);
        }
        let from = f64::from(f.h.comp.get_sidebar_slot_visual());
        assert!(from > 200.0, "scale {scale}: pinned open ({from})");
        f.key(KeyEvent::new("B", "KeyB").ctrl().shift());
        assert!(
            f.h.comp.get_sidebar_sliding() && f.h.comp.get_sidebar_collapsed(),
            "scale {scale}: sliding and collapsed at once"
        );
        // 30 ms go by before the first frame comes.
        f.clock.set(f.clock.get() + 30);
        f.h.advance(30);
        let first = f64::from(f.h.comp.get_sidebar_slot_visual());
        assert!(
            (first - from).abs() < 0.5,
            "scale {scale}: the first frame still shows the open slot ({first} of {from})"
        );
        f.step(120);
        let mid = f64::from(f.h.comp.get_sidebar_slot_visual());
        let want = from * (1.0 - css_ease(120.0 / 240.0));
        assert!(
            (mid - want).abs() < 0.5,
            "scale {scale}: 120 ms into the slide the slot is {mid}, the curve says {want}"
        );
        assert!(
            f.h.comp.get_sidebar_sliding(),
            "scale {scale}: still sliding"
        );
        f.step(120);
        let end = f64::from(f.h.comp.get_sidebar_slot_visual());
        assert!(end.abs() < 0.5, "scale {scale}: arrived ({end})");
    }
}

/// Pinning from collapsed: the terminal keeps its width through the slide, read at once after the key
/// (the e2e verticalTabs "the terminal keeps its width while they do").
#[test]
fn pinning_holds_the_terminals_width_from_the_key_on() {
    let f = Fx::with_prefs(
        Opts::default().saved(&["C:\\w\\api", "C:\\w\\web"]),
        vertical,
    );
    f.restore();
    f.key(KeyEvent::new("B", "KeyB").ctrl().shift());
    for _ in 0..30 {
        f.step(16);
    }
    f.park_pointer();
    let _ = f.h.render();
    let before = f.h.comp.get_grid_phys_width();
    assert!(f.h.comp.get_sidebar_collapsed());
    f.key(KeyEvent::new("B", "KeyB").ctrl().shift());
    assert!(f.h.comp.get_sidebar_sliding());
    assert_eq!(f.h.comp.get_grid_phys_width(), before, "at once");
    for i in 0..15 {
        f.step(16);
        let _ = f.h.render();
        assert_eq!(f.h.comp.get_grid_phys_width(), before, "frame {i}");
    }
}

/// With reduced motion the slot takes its new width at once, with no slide.
#[test]
fn the_sidebar_jumps_with_reduced_motion() {
    let f = Fx::with_prefs(
        Opts::default().saved(&["C:\\w\\api", "C:\\w\\web"]),
        vertical,
    );
    f.restore();
    slint::ComponentHandle::global::<crate::Motion>(&f.h.comp).set_reduced(true);
    for _ in 0..30 {
        f.step(16);
    }
    f.key(KeyEvent::new("B", "KeyB").ctrl().shift());
    f.h.advance(0);
    assert!(f.h.comp.get_sidebar_slot_visual().abs() < 0.5, "no slide");
    f.key(KeyEvent::new("B", "KeyB").ctrl().shift());
    f.h.advance(0);
    assert!(
        f.h.comp.get_sidebar_slot_visual() > 200.0,
        "and back at once"
    );
}

#[cfg(windows)]
#[test]
fn vertical_tabs_show_a_sidebar_with_folders_find_and_a_collapse_key() {
    let f = Fx::with_prefs(
        Opts::default().saved(&[
            "C:\\home\\mine",
            "C:\\w\\api",
            "C:\\w\\web",
            "C:\\w\\docs",
            "C:\\w\\site",
            "C:\\w\\tools",
        ]),
        vertical,
    );
    f.restore();
    assert!(f.h.comp.get_layout_vertical());
    assert_eq!(f.rows(), 6, "a six-row sidebar");
    assert!(f.row(0).folder.as_str() == "~", "the home folder reads ~");
    // The strip shows no tabs: the top row beside the buttons holds no divider but the bar.
    f.park_pointer();
    let fr = f.h.render();
    assert!(
        fr.count_where(
            f.h.phys(300.0),
            0,
            f.h.phys(800.0),
            f.h.phys(30.0),
            |p| !is_ground(p) && !Frame::near(p, [13, 13, 13, 255], 2)
        ) == 0,
        "no tab is drawn in the strip"
    );

    // Find: Ctrl+Shift+K focuses the box; typing filters; Enter fronts the first kept.
    assert_eq!(
        f.key(KeyEvent::new("K", "KeyK").ctrl().shift()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    assert!(f.h.comp.get_chrome_has_focus(), "the find box has the keys");
    f.type_text("web");
    let kept: Vec<String> = f
        .all_rows()
        .iter()
        .filter(|r| r.kept)
        .map(|r| r.label.to_string())
        .collect();
    assert_eq!(kept, ["web"]);
    f.type_text("\n");
    assert_eq!(f.front_cwd(), "C:\\w\\web");
    // Whatever Enter left in the box, Escape empties it (Enter itself clearing is tested apart).
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("\u{1b}");
    // Escape clears.
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("doc");
    assert_eq!(f.all_rows().iter().filter(|r| r.kept).count(), 1);
    f.type_text("\u{1b}");
    assert!(f.h.comp.get_sidebar_find().is_empty());
    assert!(f.all_rows().iter().all(|r| r.kept));

    // The sidebar takes room until it is collapsed; Ctrl+Shift+B collapses, Ctrl+S too.
    f.h.comp.invoke_focus_terminal();
    f.h.advance(400);
    f.park_pointer();
    let open = f.h.render();
    assert!(
        !Frame::near(open.px(f.h.phys(5.0), f.h.phys(300.0)), RED, 2),
        "the sidebar covers the left edge"
    );
    assert_eq!(sidebar_mode(&f.prefs), SidebarMode::Open);
    assert_eq!(
        f.key(KeyEvent::new("B", "KeyB").ctrl().shift()),
        KeyOutcome::Consumed
    );
    for _ in 0..30 {
        f.step(16);
    }
    f.park_pointer();
    let shut = f.h.render();
    assert_eq!(sidebar_mode(&f.prefs), SidebarMode::Collapsed);
    assert!(
        Frame::near(shut.px(f.h.phys(5.0), f.h.phys(300.0)), RED, 2),
        "a collapsed sidebar takes no room"
    );
    assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Consumed);
    assert_eq!(
        sidebar_mode(&f.prefs),
        SidebarMode::Open,
        "Ctrl+S toggles with agent keys on"
    );
    assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Consumed);
    for _ in 0..30 {
        f.step(16);
    }
    // Hover at the left edge for 60 ms reveals; leaving hides after 100 ms.
    let t = f.clock.get();
    f.ctl.borrow_mut().on_cursor(3.0, 300.0);
    f.clock.set(t + 30);
    f.ctl.borrow_mut().tick();
    assert!(!f.h.comp.get_sidebar_revealed(), "30 ms is too early");
    f.clock.set(t + 70);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_sidebar_revealed(), "60 ms reveals");
    let t2 = f.clock.get();
    f.ctl.borrow_mut().on_cursor(600.0, 300.0);
    f.clock.set(t2 + 60);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_sidebar_revealed(), "100 ms of grace");
    f.clock.set(t2 + 120);
    f.ctl.borrow_mut().tick();
    assert!(!f.h.comp.get_sidebar_revealed(), "gone after 100 ms");
}

#[cfg(not(windows))]
#[test]
fn vertical_tabs_show_a_sidebar_with_folders_find_and_a_collapse_key_posix() {
    let f = Fx::with_prefs(
        Opts::default().saved(&[
            "/Users/me/mine",
            "/w/api",
            "/w/web",
            "/w/docs",
            "/w/site",
            "/w/tools",
        ]),
        vertical,
    );
    f.restore();
    assert!(f.h.comp.get_layout_vertical());
    assert_eq!(f.rows(), 6, "a six-row sidebar");
    assert!(f.row(0).folder.as_str() == "~", "the home folder reads ~");
    // The strip shows no tabs: the top row beside the buttons holds no divider but the bar.
    f.park_pointer();
    let fr = f.h.render();
    assert!(
        fr.count_where(
            f.h.phys(300.0),
            0,
            f.h.phys(800.0),
            f.h.phys(30.0),
            |p| !is_ground(p) && !Frame::near(p, [13, 13, 13, 255], 2)
        ) == 0,
        "no tab is drawn in the strip"
    );

    // Find: Ctrl+Shift+K focuses the box; typing filters; Enter fronts the first kept.
    assert_eq!(
        f.key(KeyEvent::new("K", "KeyK").ctrl().shift()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    assert!(f.h.comp.get_chrome_has_focus(), "the find box has the keys");
    f.type_text("web");
    let kept: Vec<String> = f
        .all_rows()
        .iter()
        .filter(|r| r.kept)
        .map(|r| r.label.to_string())
        .collect();
    assert_eq!(kept, ["web"]);
    f.type_text("\n");
    assert_eq!(f.front_cwd(), "/w/web");
    // Whatever Enter left in the box, Escape empties it (Enter itself clearing is tested apart).
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("\u{1b}");
    // Escape clears.
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("doc");
    assert_eq!(f.all_rows().iter().filter(|r| r.kept).count(), 1);
    f.type_text("\u{1b}");
    assert!(f.h.comp.get_sidebar_find().is_empty());
    assert!(f.all_rows().iter().all(|r| r.kept));

    // The sidebar takes room until it is collapsed; Ctrl+Shift+B collapses, Ctrl+S too.
    f.h.comp.invoke_focus_terminal();
    f.h.advance(400);
    f.park_pointer();
    let open = f.h.render();
    assert!(
        !Frame::near(open.px(f.h.phys(5.0), f.h.phys(300.0)), RED, 2),
        "the sidebar covers the left edge"
    );
    assert_eq!(sidebar_mode(&f.prefs), SidebarMode::Open);
    assert_eq!(
        f.key(KeyEvent::new("B", "KeyB").ctrl().shift()),
        KeyOutcome::Consumed
    );
    for _ in 0..30 {
        f.step(16);
    }
    f.park_pointer();
    let shut = f.h.render();
    assert_eq!(sidebar_mode(&f.prefs), SidebarMode::Collapsed);
    assert!(
        Frame::near(shut.px(f.h.phys(5.0), f.h.phys(300.0)), RED, 2),
        "a collapsed sidebar takes no room"
    );
    assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Consumed);
    assert_eq!(
        sidebar_mode(&f.prefs),
        SidebarMode::Open,
        "Ctrl+S toggles with agent keys on"
    );
    assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Consumed);
    for _ in 0..30 {
        f.step(16);
    }
    // Hover at the left edge for 60 ms reveals; leaving hides after 100 ms.
    let t = f.clock.get();
    f.ctl.borrow_mut().on_cursor(3.0, 300.0);
    f.clock.set(t + 30);
    f.ctl.borrow_mut().tick();
    assert!(!f.h.comp.get_sidebar_revealed(), "30 ms is too early");
    f.clock.set(t + 70);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_sidebar_revealed(), "60 ms reveals");
    let t2 = f.clock.get();
    f.ctl.borrow_mut().on_cursor(600.0, 300.0);
    f.clock.set(t2 + 60);
    f.ctl.borrow_mut().tick();
    assert!(f.h.comp.get_sidebar_revealed(), "100 ms of grace");
    f.clock.set(t2 + 120);
    f.ctl.borrow_mut().tick();
    assert!(!f.h.comp.get_sidebar_revealed(), "gone after 100 ms");
}

/// Port of the verticalTabs scenario: Enter fronts the first kept row AND clears the box
/// (PT TabSidebar.tsx: `setQuery('')` before `onPick`).
#[test]
fn enter_in_the_find_box_fronts_the_first_kept_and_clears() {
    let f = Fx::with_prefs(
        Opts::default().saved(&["C:\\w\\api", "C:\\w\\web", "C:\\w\\docs"]),
        vertical,
    );
    f.restore();
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("web");
    f.type_text("\n");
    assert_eq!(f.front_cwd(), "C:\\w\\web");
    assert!(
        f.h.comp.get_sidebar_find().is_empty(),
        "Enter clears the find box"
    );
    assert!(f.all_rows().iter().all(|r| r.kept), "every row is back");
}

#[test]
fn the_slide_holds_the_terminals_size_and_fits_once() {
    let f = Fx::with_prefs(
        Opts::default().saved(&["C:\\a", "C:\\b", "C:\\c"]),
        vertical,
    );
    f.restore();
    let front = f.front_cwd();
    f.wait_made(&front);
    f.h.advance(300);
    f.pump();
    let fake = f.made_with(&front).remove(0);
    // The shell's own start-up resizes land on a worker thread: wait until they have settled.
    let mut seen = fake.resize_calls();
    let mut calm = 0;
    let t = Instant::now();
    while calm < 10 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
        f.pump();
        let now = fake.resize_calls();
        calm = if now == seen { calm + 1 } else { 0 };
        seen = now;
    }
    assert!(calm >= 10, "the start-up resizes never settled");
    let base = fake.resize_calls();
    assert_eq!(
        f.key(KeyEvent::new("B", "KeyB").ctrl().shift()),
        KeyOutcome::Consumed
    );
    let mut elapsed = 0;
    while elapsed < 240 {
        f.step(16);
        elapsed += 16;
        assert_eq!(
            fake.resize_calls(),
            base,
            "no resize during the slide ({elapsed} ms)"
        );
    }
    for _ in 0..3 {
        f.step(16);
    }
    f.step(40);
    // The fit reaches the shell on the session's resize helper thread, so on a busy runner it can land a
    // moment after the step that asked for it: wait for it (generous deadline), then hold still a while
    // in real time and check no second fit follows.
    f.pump_until("the fit after the slide", |_| fake.resize_calls() > base);
    assert_eq!(
        fake.resize_calls(),
        base + 1,
        "exactly one fit after the slide"
    );
    f.step(100);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(20));
        f.pump();
    }
    assert_eq!(fake.resize_calls(), base + 1);
}

#[test]
fn a_row_dragged_over_the_next_swaps_and_a_filtered_list_refuses() {
    let f = Fx::with_prefs(
        Opts::default().saved(&["C:\\w\\a1", "C:\\w\\a2", "C:\\w\\a3", "C:\\w\\a4"]),
        vertical,
    );
    f.restore();
    let (x, y) = f.side_row_center(1);
    f.h.move_to(x, y);
    f.h.press(x, y, PointerEventButton::Left);
    for i in 1..=6 {
        f.h.move_to(x, y + 10.0 * i as f32);
        f.h.advance(0);
    }
    assert!(f.h.comp.get_carrying());
    f.h.release(x, y + 60.0, PointerEventButton::Left);
    f.h.advance(0);
    assert_eq!(
        f.cwds(),
        ["C:\\w\\a1", "C:\\w\\a3", "C:\\w\\a2", "C:\\w\\a4"]
    );

    // A typed query: the list refuses the carry.
    f.key(KeyEvent::new("K", "KeyK").ctrl().shift());
    f.type_text("a");
    let before = f.cwds();
    let (x, y) = f.side_row_center(1);
    f.h.move_to(x, y);
    f.h.press(x, y, PointerEventButton::Left);
    for i in 1..=6 {
        f.h.move_to(x, y + 10.0 * i as f32);
        f.h.advance(0);
    }
    assert!(!f.h.comp.get_carrying(), "no carry under a query");
    f.h.release(x, y + 60.0, PointerEventButton::Left);
    f.h.advance(0);
    assert_eq!(f.cwds(), before);
}

#[test]
fn plus_opens_at_once_in_the_chosen_folder_and_does_not_ask_again() {
    let f = Fx::with_prefs(Opts::default().saved(&["C:\\a"]), |p| {
        set_new_tab_folder(p, "D:\\w");
        remember_root(p, "C:\\r1");
        remember_root(p, "C:\\r2");
    });
    f.restore();
    let (px, py) = f.plus_center();
    f.h.click(px, py);
    f.pump_until("the tab", |f| f.rows() == 2);
    assert_eq!(f.cwds(), ["C:\\a", "D:\\w"]);
    assert!(
        f.log.borrow().picks.is_empty(),
        "the picker was never called"
    );

    // Right click (the strip's + keeps it): the folder menu, "Choose a folder…" first, then the pins.
    let (px, py) = f.plus_center();
    f.h.right_click(px, py);
    f.h.advance(0);
    assert!(f.h.comp.get_menu_open());
    let labels = |f: &Fx| -> Vec<String> {
        let rows = f.h.comp.get_menu_rows();
        (0..rows.row_count())
            .map(|i| rows.row_data(i).unwrap().label.to_string())
            .collect()
    };
    let before = labels(&f);
    assert_eq!(before[0], crate::menus_ui::CHOOSE_FOLDER);
    assert!(before.len() >= 4, "recents are listed: {before:?}");
    let last = before.len() - 1;
    // Pinning the last row keeps the menu open and moves that row to the top.
    let last_name = before[last].clone();
    f.h.comp.invoke_menu_trailing(last as i32);
    f.h.advance(0);
    assert!(f.h.comp.get_menu_open(), "the menu stays open");
    assert_eq!(labels(&f)[1], last_name, "the pinned row climbs");
    assert_eq!(pinned_roots(&f.prefs).len(), 1);
    // Clicking the second row (the first folder, after the group's 9 px gap) opens a tab there.
    let rows_before = f.rows();
    let panel_y = py.min(f.css.1 as f32);
    let row_y = panel_y + f.hair_phys() as f32 / f.scale + 28.0 + 9.0 + 28.0 / 2.0;
    f.h.click(px + 20.0, row_y);
    f.pump_until("the menu tab", |f| f.rows() == rows_before + 1);
    assert!(!f.h.comp.get_menu_open());

    // No recents at all: the first row, then one disabled note.
    for r in recent_roots(&f.prefs) {
        forget_root(&f.prefs, &r);
    }
    for r in pinned_roots(&f.prefs) {
        forget_root(&f.prefs, &r);
        flare_settings::recent_roots::toggle_pin(&f.prefs, &r);
    }
    let (px, py) = f.plus_center();
    f.h.right_click(px, py);
    f.h.advance(0);
    assert!(f.h.comp.get_menu_open());
    assert_eq!(
        labels(&f),
        [crate::menus_ui::CHOOSE_FOLDER, "No recent folders"]
    );
    assert!(f.h.comp.get_menu_rows().row_data(1).unwrap().disabled);
}

#[test]
fn a_right_click_outside_an_open_menu_opens_the_menu_of_the_tab_under_it() {
    for s in [1.0_f32, 2.25] {
        let f = Fx::new(Opts::default().saved(&[r"C:\a", r"C:\b", r"C:\c"]).scale(s));
        f.restore();
        // A is the last tab: its menu hangs right and down, away from tab B on its left.
        let (ax, ay) = f.tab_center(2);
        let (bx, by) = f.tab_center(0);
        f.h.right_click(ax, ay);
        f.h.advance(0);
        assert!(f.h.comp.get_menu_open(), "scale {s}");
        assert!((f.h.comp.get_menu_x() - ax).abs() < 1.0, "scale {s}");
        // Another tab: the one click closes menu A and opens tab B's, hung at the new point.
        f.h.right_click(bx, by);
        f.pump();
        assert!(
            f.h.comp.get_menu_open(),
            "scale {s}: reopened, not toggled shut"
        );
        assert!((f.h.comp.get_menu_x() - bx).abs() < 1.0, "scale {s}");
        // The menu belongs to tab B: its folder is what Copy folder path would copy.
        let rows = f.h.comp.get_menu_rows();
        assert_eq!(
            rows.row_data(1).unwrap().label,
            "Copy folder path",
            "scale {s}"
        );
        // The + button is a different surface with its own menu: the places list.
        let (px, py) = f.plus_center();
        f.h.right_click(px, py);
        f.pump();
        assert!(f.h.comp.get_menu_open(), "scale {s}");
        assert!((f.h.comp.get_menu_x() - px).abs() < 1.0, "scale {s}");
        assert_ne!(
            f.h.comp.get_menu_rows().row_data(0).unwrap().label,
            "Close tab",
            "scale {s}: the + menu, not a tab menu"
        );
        // A left press outside only closes.
        let (cx, cy) = f.tab_center(2);
        f.h.click(cx, cy);
        f.pump();
        assert!(!f.h.comp.get_menu_open(), "scale {s}");
        assert_eq!(f.front_cwd(), r"C:\a", "scale {s}: it picked no tab");
    }
}

#[test]
fn a_right_click_outside_the_plus_n_list_closes_it_and_opens_the_tab_menu_under_it() {
    let all = names(16);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let f = Fx::new(Opts::default().size(700, 600).saved(&refs));
    f.restore();
    let (mx, my) = f.more_center();
    f.h.click(mx, my);
    f.h.advance(0);
    assert!(f.h.comp.get_overflow_open());
    let (tx, ty) = f.tab_center(0);
    f.h.right_click(tx, ty);
    f.pump();
    assert!(!f.h.comp.get_overflow_open(), "the list closed");
    assert!(f.h.comp.get_menu_open(), "the tab menu opened");
    assert!((f.h.comp.get_menu_x() - tx).abs() < 1.0);
}

#[test]
fn closing_the_plus_n_list_by_a_right_click_on_a_bare_spot_gives_the_keys_back() {
    let all = names(16);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let f = Fx::new(Opts::default().size(700, 600).saved(&refs));
    f.restore();
    let (mx, my) = f.more_center();
    f.h.click(mx, my);
    f.h.advance(0);
    assert!(f.h.comp.get_overflow_open());
    // The bar's far left, over no tab and no button.
    f.h.right_click(1.0, 1.0);
    f.pump();
    assert!(!f.h.comp.get_overflow_open());
    assert!(
        !f.h.comp.get_chrome_has_focus(),
        "no Slint control keeps the keys: they reach the terminal"
    );
}

#[test]
fn a_right_click_outside_a_settings_dropdown_closes_it_and_opens_the_tab_menu() {
    let f = Fx::new(Opts::default().saved(&[r"C:", r"C:", r"C:\c"]));
    f.restore();
    let (tx, ty) = f.tab_center(0);
    let b = slint::ComponentHandle::global::<crate::SettingsBridge>(&f.h.comp);
    // The Settings controller is not part of this fixture: stand in for its close.
    b.on_select_dismissed({
        let w = slint::ComponentHandle::as_weak(&f.h.comp);
        move || {
            if let Some(a) = w.upgrade() {
                slint::ComponentHandle::global::<crate::SettingsBridge>(&a).set_select_open(false);
            }
        }
    });
    b.set_select_x(300.0);
    b.set_select_y(200.0);
    b.set_select_w(160.0);
    b.set_select_h(80.0);
    b.set_select_open(true);
    f.h.advance(0);
    f.h.right_click(tx, ty);
    f.pump();
    assert!(!b.get_select_open(), "the dropdown closed");
    assert!(
        f.h.comp.get_menu_open(),
        "the tab menu opened under the pointer"
    );
    assert!((f.h.comp.get_menu_x() - tx).abs() < 1.0);
}

#[test]
fn exit_ends_its_tab_and_ctrl_w_closes_the_front() {
    let f = Fx::new(Opts::default().saved(&["C:\\a", "C:\\b"]));
    f.restore();
    f.wait_made("C:\\a");
    f.wait_made("C:\\b");
    assert_eq!(f.front_cwd(), "C:\\a");
    f.made_with("C:\\a")[0].exit(0);
    f.pump_until("the tab to go", |f| f.rows() == 1);
    assert_eq!(f.cwds(), ["C:\\b"]);
    assert_eq!(f.front_cwd(), "C:\\b");
    f.ctrl_t();
    f.pump_until("the new tab", |f| f.rows() == 2);
    assert_eq!(f.front_cwd(), HOME);
    assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert_eq!(f.cwds(), ["C:\\b"]);
    assert_eq!(f.front_cwd(), "C:\\b");
}

#[test]
fn two_saved_tabs_return_in_their_folders_in_order() {
    let f = Fx::new(Opts::default().saved(&["C:\\one", "C:\\two"]));
    f.restore();
    f.pump_until("both shells", |f| f.sp.made().len() >= 2);
    assert_eq!(f.cwds(), ["C:\\one", "C:\\two"]);
    // One shell per saved tab, each in its own folder (a spare shell warmed in the new-tab
    // folder is not a tab).
    for want in ["C:\\one", "C:\\two"] {
        f.pump_until("a spawn", |f| f.made_with(want).len() == 1);
    }
    let rows = f.all_rows();
    assert!(rows[0].x < rows[1].x, "the strip keeps the saved order");
    assert!(rows[0].active, "the saved front is in front");
}

#[test]
fn the_first_frame_has_the_tabs_and_the_ring_on_agent_tabs() {
    let f = Fx::new(Opts {
        saved: vec![
            ("C:\\a".to_string(), None),
            ("C:\\b".to_string(), Some(AgentKind::Claude)),
        ],
        ..Opts::default()
    });
    // Before the restore answered: placeholders at the first render.
    f.park_pointer();
    let fr = f.h.render();
    let rows = f.all_rows();
    assert_eq!(rows.len(), 2);
    assert!(rows[1].loading && !rows[0].loading);
    let edges = f.divider_edges(&fr);
    assert_eq!(
        edges.len(),
        3,
        "the tab dividers are drawn at the first render: {edges:?}"
    );
    assert_eq!(f.h.comp.get_front_kind(), 3);
    assert_eq!(f.sp.made().len(), 0, "no shell yet");
    let page = |fr: &Frame| {
        fr.count_where(
            f.h.phys(100.0),
            f.h.phys(100.0),
            f.h.phys(900.0),
            f.h.phys(600.0),
            |p| !is_ground(p),
        )
    };
    assert_eq!(page(&fr), 0, "the front shows the bare ground");
    let ring = |fr: &Frame, r: &TabRow| {
        let left = f.strip_left_phys();
        fr.count_where(
            left + (r.x + r.w) as u32 - f.h.phys(40.0),
            f.h.phys(6.0),
            left + (r.x + r.w) as u32 - f.h.phys(8.0),
            f.h.phys(30.0),
            |p| Frame::near(p, RING, 40),
        )
    };
    assert!(ring(&fr, &rows[1]) > 0, "the agent tab shows its ring");
    f.restore();
    f.park_pointer();
    let fr = f.h.render();
    let rows = f.all_rows();
    assert!(rows.iter().all(|r| !r.loading), "the ring is gone");
    assert_eq!(ring(&fr, &rows[1]), 0);
}

#[test]
fn a_handed_folder_is_a_new_tab_even_when_shown_already() {
    let f = Fx::new(Opts {
        real_fs: true,
        ..Opts::default()
    });
    let sub = f.dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap(); // sync-ok: test code only, never the app
    f.restore();
    f.open_via_handoff("sub");
    f.pump_until("the tab", |f| f.rows() == 1);
    f.open_via_handoff("sub");
    f.pump_until("the second tab", |f| f.rows() == 2);
    assert_eq!(f.cwds()[0], f.cwds()[1]);
    assert_eq!(Path::new(&f.cwds()[1]), sub.as_path());
    assert_eq!(f.log.borrow().raised, 2);
}

fn open_agent_tab(f: &Fx) -> String {
    f.ctrl_t();
    f.pump_until("the tab", |f| f.rows() == 1);
    let id = f.ctl.borrow().model().active().unwrap().id.clone();
    f.ctl
        .borrow_mut()
        .model_mut()
        .set_agent(&id, Some(AgentKind::Claude));
    id
}

#[test]
fn a_tab_with_an_agent_asks_and_cancel_keeps_it() {
    let f = Fx::new(Opts::default());
    f.restore();
    let id = open_agent_tab(&f);
    // The question names the agent (and, when it works, how long).
    assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(
        f.h.comp.get_dialog_title().as_str(),
        "Close the tab and end the agent?"
    );
    let body = |working: Option<u64>| {
        let label = f.ctl.borrow().model().labels()[0].clone();
        ask_view(
            &tab_ask(
                &id,
                &label,
                flare_agent::kinds::DetectedAgent::Claude,
                working,
            ),
            flare_tabs::close_flow::NAME_INK,
        )
        .body
    };
    assert!(body(None).contains("Claude"), "{}", body(None));
    assert!(
        body(Some(65_000)).contains("working for"),
        "{}",
        body(Some(65_000))
    );
    // Tab moves between the two buttons and never leaves: one Tab lands on Cancel.
    f.type_text("\t");
    f.type_text("\n");
    assert!(!f.h.comp.get_dialog_open(), "Enter on Cancel cancels");
    assert_eq!(f.rows(), 1, "the tab stays");
    // Two Tabs are back on the primary; Shift+Tab from the primary lands on Cancel.
    f.key(ctrl("w", "KeyW"));
    f.type_text("\t");
    f.type_text("\t");
    f.type_text("\n");
    assert!(!f.h.comp.get_dialog_open(), "confirm closes");
    assert_eq!(f.rows(), 0);
    let g = Fx::new(Opts::default());
    g.restore();
    let _ = open_agent_tab(&g);
    g.key(ctrl("w", "KeyW"));
    g.type_text("\u{19}");
    g.type_text("\n");
    assert_eq!(g.rows(), 1, "Shift+Tab from the primary lands on Cancel");
    // Escape cancels.
    g.key(ctrl("w", "KeyW"));
    assert!(g.h.comp.get_dialog_open());
    g.type_text("\u{1b}");
    assert!(!g.h.comp.get_dialog_open());
    assert_eq!(g.rows(), 1);
    // A press over the title bar cancels.
    g.key(ctrl("w", "KeyW"));
    assert!(g.h.comp.get_dialog_open());
    g.h.click(300.0, 10.0);
    g.h.advance(0);
    assert!(!g.h.comp.get_dialog_open());
    assert_eq!(g.rows(), 1);
    // The working variant.
    {
        let mut c = g.ctl.borrow_mut();
        let id = c.model().active().unwrap().id.clone();
        c.set_working(&id, Some(0));
    }
    g.clock.set(65_000);
    g.key(ctrl("w", "KeyW"));
    assert!(g.h.comp.get_dialog_open());
    g.h.comp.invoke_dialog_confirm();
    assert_eq!(g.rows(), 0);
    assert!(f.nothing_was_typed() && g.nothing_was_typed());
}

#[test]
fn ctrl_t_and_ctrl_tab_do_nothing_under_the_question() {
    let f = Fx::new(Opts::default().saved(&["C:\\a", "C:\\b"]));
    f.restore();
    f.wait_made("C:\\a");
    let id = f.ctl.borrow().model().active().unwrap().id.clone();
    f.ctl
        .borrow_mut()
        .model_mut()
        .set_agent(&id, Some(AgentKind::Claude));
    f.key(ctrl("w", "KeyW"));
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    assert_eq!(
        f.key(KeyEvent::new("Tab", "Tab").ctrl()),
        KeyOutcome::Consumed
    );
    assert_eq!(f.rows(), 2, "no tab was opened");
    assert_eq!(f.front_cwd(), "C:\\a", "no tab was stepped to");
    assert!(f.nothing_was_typed(), "the keys never reached the pty");
}

#[test]
fn tabs_in_the_top_row_buttons_at_the_right_one_row_above_the_terminal() {
    for s in [1.0_f32, 1.5, 2.25] {
        let f = Fx::new(Opts::default().scale(s).saved(&["C:\\a", "C:\\b", "C:\\c"]));
        f.restore();
        f.pump_until("a shell", |f| !f.sp.made().is_empty());
        f.park_pointer();
        let fr = f.h.render();
        // The terminal (the test's red page) starts one 36 css px row down.
        let page = fr
            .bbox_where(0, 0, fr.w, fr.h, |p| Frame::near(p, RED, 2))
            .expect("page pixels");
        assert_eq!(
            page.1,
            (36.0 * s).round() as u32,
            "scale {s}: the page is one row down"
        );
        // The buttons are at the right end: close is the last 32 css px (+6 padding).
        let (cx, cy) = f.button_center(0);
        f.h.click(cx, cy);
        let (mx, my) = f.button_center(1);
        f.h.click(mx, my);
        let (nx, ny) = f.button_center(2);
        f.h.click(nx, ny);
        assert_eq!(f.log.borrow().close_clicks, 1, "scale {s}");
        assert_eq!(f.log.borrow().maximize_clicks, 1, "scale {s}");
        assert_eq!(f.log.borrow().minimize_clicks, 1, "scale {s}");
        // Empty strip space is not a tab: a press there picks nothing and closes a menu.
        let (tx, ty) = f.tab_center(1);
        f.h.click(tx, ty);
        f.h.right_click(tx, ty);
        f.h.advance(0);
        assert!(f.h.comp.get_menu_open(), "scale {s}");
        let g = f.h.comp.get_strip_geom();
        let empty = (f.strip_left_phys() as f32 + (g.plus_x + g.plus_w) as f32) / s + 60.0;
        f.h.click(empty, ty);
        f.h.advance(0);
        assert!(
            !f.h.comp.get_menu_open(),
            "scale {s}: the empty strip space is the bar"
        );
        assert_eq!(f.front_cwd(), "C:\\b", "scale {s}: it picked no tab");
        assert!(!f.h.comp.get_carrying());
    }
    // With no tabs the row still holds the buttons.
    let f = Fx::new(Opts::default());
    f.restore();
    assert_eq!(f.rows(), 0);
    let (cx, cy) = f.button_center(0);
    f.h.click(cx, cy);
    assert_eq!(f.log.borrow().close_clicks, 1);
}

#[test]
fn a_tip_waits_a_second_and_is_centred_under_the_tab() {
    for s in [1.0_f32, 2.25] {
        let f = Fx::new(
            Opts::default()
                .scale(s)
                .saved(&["C:\\w\\a", "C:\\w\\bravo", "C:\\w\\c"]),
        );
        f.restore();
        let (x, y) = f.tab_center(1);
        let rows = f.all_rows();
        let left = f.strip_left_phys() as f32;
        let tab_mid = (left + rows[1].x as f32 + rows[1].w as f32 / 2.0) / s;
        let tab_bottom = BAR_H - f.hair_phys() as f32 / s;
        f.h.move_to(x, y);
        f.h.advance(0);
        f.clock.set(f.clock.get() + 999);
        f.ctl.borrow_mut().tick();
        assert!(
            !f.h.comp.get_tip_visible(),
            "scale {s}: 999 ms is too early"
        );
        f.clock.set(f.clock.get() + 2);
        f.ctl.borrow_mut().tick();
        assert!(f.h.comp.get_tip_visible(), "scale {s}: 1001 ms shows it");
        assert_eq!(f.h.comp.get_tip_text().as_str(), "C:\\w\\bravo");
        let centre = f.h.comp.get_tip_x() + f.h.comp.get_tip_w() / 2.0;
        assert!(
            (centre - tab_mid).abs() <= 1.0,
            "scale {s}: tip centre {centre} vs {tab_mid}"
        );
        assert_eq!(f.h.comp.get_tip_side(), 0, "below the tab");
        let gap = f.h.comp.get_tip_y() - tab_bottom;
        assert!(
            (gap - 6.0).abs() <= 1.0,
            "scale {s}: {gap} css px below the tab"
        );
        // A press clears it.
        f.h.click(x, y);
        f.h.advance(0);
        assert!(
            !f.h.comp.get_tip_visible(),
            "scale {s}: a press clears the tip"
        );
    }
}

#[test]
fn the_strip_and_the_sidebar_work_at_every_scale() {
    for s in SCALES {
        // The strip: click the second tab, middle-click the third.
        let f = Fx::new(Opts::default().scale(s).saved(&["C:\\a", "C:\\b", "C:\\c"]));
        f.restore();
        let (x, y) = f.tab_center(1);
        f.h.click(x, y);
        f.h.advance(0);
        assert_eq!(f.front_cwd(), "C:\\b", "strip, scale {s}");
        let (x, y) = f.tab_center(2);
        f.h.middle_click(x, y);
        f.h.advance(0);
        assert_eq!(f.cwds(), ["C:\\a", "C:\\b"], "strip, scale {s}");
        assert_eq!(f.front_cwd(), "C:\\b");

        // The sidebar: the same two gestures, by the row grid.
        let g = Fx::with_prefs(
            Opts::default().scale(s).saved(&["C:\\a", "C:\\b", "C:\\c"]),
            vertical,
        );
        g.restore();
        let (x, y) = g.side_row_center(1);
        g.h.click(x, y);
        g.h.advance(0);
        assert_eq!(g.front_cwd(), "C:\\b", "sidebar, scale {s}");
        let (x, y) = g.side_row_center(2);
        g.h.middle_click(x, y);
        g.h.advance(0);
        assert_eq!(g.cwds(), ["C:\\a", "C:\\b"], "sidebar, scale {s}");
    }
}
