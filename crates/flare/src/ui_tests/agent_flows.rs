//! Agent indicator flows through the real window (Task 31): one `Harness<AppWindow>` plus an
//! `AppCtl` over fakes per test. The stand-in agents speak the real protocol: a fake shell
//! "prints" `ESC ] 777 ; flare-agent ; state=... BEL` and titles `ESC ] 0 ; ... BEL`, paints the
//! question box rows, and the tests read the rows the window pushed and the pixels it drew.
//! Pointer positions come from the rendered layout (the divider columns of the strip row), never
//! from magic numbers. The test names carry the e2e scenario of `docs/parity.md` they port.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_agent::kinds::{AgentKind, DetectedAgent};
use flare_agent::mark_motion::mark_phase;
use flare_agent::question_widget::ShownQuestion;
use flare_diag::agent_diag::MemorySink;
use flare_settings::agent_prefs::{
    set_agent_done_on, set_agent_failed_on, set_agent_guard, set_agent_rainbow,
};
use flare_settings::kv::PrefStore;
use flare_settings::look::{AgentIndicator, LookStore};
use flare_settings::tab_layout::{TabLayout, set_tab_layout};
use flare_settings::term_prefs::set_agent_keys;
use flare_shells::list::ShellDef;
use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, SessionLookup, tabs_json};
use flare_tabs::tabs_store::TabsStore;
use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
use flare_term::keys::KeyEvent;
use flare_win::clipboard_port::MemoryClipboard;
use flare_win::taskbar::{Disc, OverlayShell, TaskbarBadge};
use flare_win::window_state::WindowStateStore;
use slint::{Color, ComponentHandle, Model};

use crate::appctl::{AppCtl, Deps, KeyOutcome, Post, TermPorts, UiMsg};
use crate::sessions::FixedSessions;
use crate::tab_host::TabHost;
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, MarkMotion, TabRow};

mod convo_flows;
mod remote_flows;
mod stage7_flows;

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;
type Posts = Arc<Mutex<Vec<(String, ShownQuestion)>>>;

/// The strip divider over `Theme.ground`.
const DIVIDER: [u8; 4] = [39, 39, 39, 255];
const BAR_H: f32 = 36.0;
const HOME: &str = "C:\\home";

const W: &str = "\x1b]777;flare-agent;state=working\x07";
const Q: &str = "\x1b]777;flare-agent;state=question\x07";
const D: &str = "\x1b]777;flare-agent;state=done\x07";
const F: &str = "\x1b]777;flare-agent;state=failed;kind=rate_limit\x07";

const T_IDLE: &str = "\u{2733} Claude Code";
const T_SPIN: &str = "\u{25d0} Claude Code";
const T_CODEX_ASKS: &str = "[ ! ] Action Required | proj";

const SESSION_A: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

/// The ground-up question tool's box (Claude's screen).
const QUESTION_BOX: [&str; 7] = [
    "\u{25cf} Which colour do you prefer?",
    "",
    "  \u{276f} 1. Red",
    "    2. Blue",
    "    3. Type something.",
    "",
    "Enter to select \u{b7} \u{2191}/\u{2193} to navigate \u{b7} Esc to cancel",
];

/// A permission box.
const PERMISSION_BOX: [&str; 9] = [
    " Bash command",
    "",
    "   echo hello-signals",
    "   Print a test string",
    "",
    " Do you want to proceed?",
    " \u{276f} 1. Yes",
    "   2. Yes, and don't ask again for echo commands in this folder",
    "   3. No, and tell Claude what to do differently (esc)",
];

fn title(t: &str) -> String {
    format!("\x1b]0;{t}\x07")
}

/// Clear the screen and paint `rows` from the top (the way Claude's TUI repaints: the display and
/// the scrollback are erased, because alacritty keeps a cleared screen in its history).
fn paint(rows: &[&str]) -> String {
    format!("{}{}", clear(), rows.join("\r\n"))
}

fn clear() -> &'static str {
    "\x1b[2J\x1b[3J\x1b[H"
}

fn screens(name: &str) -> Vec<String> {
    let all: serde_json::Value = serde_json::from_str(include_str!(
        "../../../flare-agent/tests/fixtures/question_widget_screens.json"
    ))
    .expect("fixture parses");
    all[name]
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("row").to_string())
        .collect()
}

fn paint_owned(rows: &[String]) -> String {
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    paint(&refs)
}

fn rgba(c: Color) -> [u8; 4] {
    [c.red(), c.green(), c.blue(), 255]
}

/// Every overlay the taskbar was given: the disc size (None: cleared) and the words.
type Overlay = (Option<u32>, String);

#[derive(Clone, Default)]
struct BadgeLog(Rc<RefCell<Vec<Overlay>>>);

impl BadgeLog {
    fn last(&self) -> Option<Overlay> {
        self.0.borrow().last().cloned()
    }
}

struct RecShell(BadgeLog);

impl OverlayShell for RecShell {
    fn set_overlay(&mut self, icon: Option<&Disc>, text: &str) -> bool {
        (self.0)
            .0
            .borrow_mut()
            .push((icon.map(|d| d.size), text.to_string()));
        true
    }
}

#[derive(Default)]
struct Opts {
    scale: f32,
    saved: Vec<(&'static str, Option<AgentKind>)>,
    lookup: Option<Arc<dyn SessionLookup>>,
    shells: Option<Vec<ShellDef>>,
}

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<AppCtl>>,
    q: Queue,
    sp: Arc<FakeSpawner>,
    _dir: tempfile::TempDir,
    clock: Rc<Cell<u64>>,
    prefs: PrefStore,
    clip: Arc<MemoryClipboard>,
    posts: Posts,
    diag: Arc<MemorySink>,
    badge: BadgeLog,
    scale: f32,
}

fn grid_image() -> slint::Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(8, 8);
    for p in buf.make_mut_slice() {
        *p = slint::Rgba8Pixel {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
    }
    slint::Image::from_rgba8(buf)
}

impl Fx {
    fn new() -> Fx {
        Self::with(Opts::default(), |_| {})
    }

    fn with(opts: Opts, prep: impl FnOnce(&PrefStore)) -> Fx {
        let scale = if opts.scale > 0.0 { opts.scale } else { 1.0 };
        let dir = tempfile::tempdir().unwrap();
        if !opts.saved.is_empty() {
            let saved = SavedTabs {
                tabs: opts
                    .saved
                    .iter()
                    .map(|(cwd, agent)| SavedTab {
                        cwd: (*cwd).to_string(),
                        agent: *agent,
                        remote: None,
                    })
                    .collect(),
                active: 0,
            };
            std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test code only, never the app
        }
        let prefs = PrefStore::memory();
        // The strip along the top: these tests were written for it. Vertical is the default
        // since 2026-11-11; a test of the default clears the key in its `prep`.
        set_tab_layout(&prefs, TabLayout::Horizontal);
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
        let shells = match opts.shells.clone() {
            Some(defs) => FakeShells::with(defs),
            None => FakeShells::new(),
        };
        let host = TabHost::new(sp.clone(), shells, prefs.clone(), waker, None);
        let clip = Arc::new(MemoryClipboard::new());
        let clock = Rc::new(Cell::new(0u64));
        let mut term = TermPorts::inert();
        term.clip_port = clip.clone();
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
            lookup: opts.lookup.clone().unwrap_or_else(|| Arc::new(NoSessions)),
            is_dir: Arc::new(|p| !p.contains("gone")),
            home: HOME.to_string(),
            clock: {
                let c = clock.clone();
                Rc::new(move || c.get())
            },
            post,
            pick_folder: Rc::new(|_, done| done(None)),
            clipboard: Rc::new(|_| {}),
            open_url: Rc::new(|_| {}),
            set_grid_source: Rc::new(|_| {}),
            raise: Rc::new(|| {}),
            quit: Rc::new(|| {}),
            hide: Rc::new(|| {}),
            starter_folders: Rc::new(|_| {}),
            launch_folders: Vec::new(),
            e2e: true,
            term,
        };
        let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
        h.comp.set_grid_image(grid_image());
        let ctl = AppCtl::new(&h.comp, deps);
        // Pinned: the runner's Windows "show animations" switch must not decide the clock or the marks (the help panel and chrome motion still read the OS).
        ctl.borrow_mut().set_animations_source(Rc::new(|| true));
        {
            let weak = Rc::downgrade(&ctl);
            h.comp.on_grid_resized(move |w, hgt, _scale| {
                if let Some(rc) = weak.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.on_grid_dims((w / 8).max(2) as usize, (hgt / 16).max(1) as usize);
                }
            });
        }
        let posts: Posts = Arc::default();
        let diag = Arc::new(MemorySink::default());
        {
            let posts = posts.clone();
            ctl.borrow_mut().hub_mut().set_sinks(
                Arc::new(move |token, q| posts.lock().unwrap().push((token, q))),
                diag.clone(),
            );
        }
        let badge = BadgeLog::default();
        ctl.borrow_mut()
            .set_taskbar_badge(TaskbarBadge::with_shell(Box::new(RecShell(badge.clone()))));
        h.advance(0);
        ctl.borrow_mut().refresh();
        Fx {
            h,
            ctl,
            q,
            sp,
            _dir: dir,
            clock,
            prefs,
            clip,
            posts,
            diag,
            badge,
            scale,
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
        let shells: Vec<String> = self
            .sp
            .made()
            .iter()
            .map(|m| m.spec().cwd.clone())
            .collect();
        panic!(
            "timed out waiting for {what}; tabs {:?}, shells {shells:?}",
            self.cwds()
        );
    }

    /// The window clock and the Slint animation clock move together; the controller ticks and the
    /// shells' output is applied.
    fn step(&self, ms: u64) {
        self.pump();
        self.clock.set(self.clock.get() + ms);
        self.h.advance(ms);
        self.ctl.borrow_mut().tick();
        self.pump();
    }

    /// Waits (real time, short) until the fake shells' output reached the window.
    fn settle_output(&self) {
        // The shell's reader thread wakes the window when its output is ready; wait for that wake
        // (an output that makes no wake is simply drained by the pump).
        let t = Instant::now();
        while self.q.lock().unwrap().is_empty() && t.elapsed() < Duration::from_millis(400) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.pump();
    }

    /// Steps the fake clock in `ms` slices (at most `limit_ms` in all) until `cond` holds.
    fn until_step(&self, what: &str, ms: u64, limit_ms: u64, mut cond: impl FnMut(&Fx) -> bool) {
        let mut spent = 0;
        loop {
            self.step(0);
            if cond(self) {
                return;
            }
            assert!(
                spent <= limit_ms,
                "{what} did not happen within {limit_ms} ms"
            );
            self.step(ms);
            spent += ms;
        }
    }

    /// Ticks the controller in real time (the resume reveal's own clock) until `cond` holds.
    fn tick_until(&self, what: &str, mut cond: impl FnMut(&Fx) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            self.pump();
            self.ctl.borrow_mut().tick();
            if cond(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    /// For `ms` of fake time in `slice` steps, `cond` holds at every step.
    fn holds_for(&self, what: &str, slice: u64, ms: u64, mut cond: impl FnMut(&Fx) -> bool) {
        let mut spent = 0;
        loop {
            self.step(0);
            assert!(cond(self), "{what} failed at {spent} ms");
            if spent >= ms {
                return;
            }
            self.step(slice);
            spent += slice;
        }
    }

    fn row(&self, i: usize) -> TabRow {
        self.h.comp.get_tab_rows().row_data(i).expect("a row")
    }

    fn rows(&self) -> usize {
        self.h.comp.get_tab_rows().row_count()
    }

    fn ids(&self) -> Vec<String> {
        self.ctl
            .borrow()
            .model()
            .state
            .tabs
            .iter()
            .map(|t| t.id.clone())
            .collect()
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

    fn front(&self) -> String {
        self.ctl
            .borrow()
            .model()
            .state
            .active_id
            .clone()
            .expect("a front tab")
    }

    fn shell(&self, cwd: &str) -> Arc<FakeState> {
        self.sp
            .made()
            .into_iter()
            .find(|m| m.spec().cwd == cwd)
            .unwrap_or_else(|| panic!("no shell in {cwd}"))
    }

    /// What the shell of `cwd` prints (an agent's hook signal, a title, a screen).
    fn say(&self, cwd: &str, text: &str) {
        self.shell(cwd).feed(text.as_bytes());
        self.settle_output();
    }

    /// Opens a tab per path (the last in front); the user is looking elsewhere (nowhere).
    fn tabs(&self, paths: &[&str]) -> Vec<String> {
        let mut ids = Vec::new();
        for p in paths {
            ids.push(self.ctl.borrow_mut().open_folder(p));
            self.pump_until("a spawn", |f| {
                f.sp.made().iter().any(|m| m.spec().cwd == *p)
            });
        }
        self.ctl.borrow_mut().hub_mut().set_looking(None, false);
        self.pump();
        ids
    }

    /// The Settings tab, opened in front over the tabs (the real controller over the same store).
    fn open_settings(&self) -> Rc<RefCell<crate::settings_ctl::SettingsCtl>> {
        let post: Post = {
            let q = self.q.clone();
            Arc::new(move |m| q.lock().unwrap().push_back(m))
        };
        let clock = self.clock.clone();
        let settings = crate::settings_ctl::SettingsCtl::new(
            &self.h.comp,
            crate::settings_scene::deps(&self.prefs, post, Rc::new(move || clock.get())),
        );
        {
            let mut c = self.ctl.borrow_mut();
            c.set_settings(settings.clone());
            c.model_mut().open_settings();
            c.refresh();
        }
        self.pump();
        assert_eq!(self.h.comp.get_front_kind(), 1, "Settings is in front");
        settings
    }

    /// A Settings switch, as its control fires it.
    fn settings_switch(&self, row: &str, on: bool) {
        self.h
            .comp
            .global::<crate::SettingsBridge>()
            .invoke_row_switch(row.into(), on);
        self.pump();
    }

    /// The user looks at tab `i`: it comes to the front and the window is watched.
    fn visit(&self, i: usize) {
        let id = self.ids()[i].clone();
        {
            let mut c = self.ctl.borrow_mut();
            c.model_mut().pick(&id);
            c.refresh();
            c.hub_mut().set_looking(Some(&id), true);
        }
        self.pump();
    }

    fn working(&self, i: usize) -> bool {
        let id = self.ids()[i].clone();
        self.ctl.borrow_mut().hub_mut().working(&id)
    }

    fn agent_here(&self, i: usize) -> bool {
        let id = self.ids()[i].clone();
        self.ctl.borrow_mut().hub_mut().agent_here(&id)
    }

    fn in_set(&self, which: &str, i: usize) -> bool {
        let id = self.ids()[i].clone();
        let mut c = self.ctl.borrow_mut();
        let ind = c.hub_mut().indicator();
        match which {
            "working" => ind.working_ids().contains(&id),
            "question" => ind.question_ids().contains(&id),
            "done" => ind.done_ids().contains(&id),
            "failed" => ind.failed_ids().contains(&id),
            _ => panic!("no set {which}"),
        }
    }

    fn set_indicator(&self, v: AgentIndicator) {
        LookStore::new(self.prefs.clone()).set_agent_indicator(v);
        self.pump();
    }

    fn motion(&self) -> (bool, f32, f32, f32, f32) {
        let g = self.h.comp.global::<MarkMotion>();
        (
            g.get_still(),
            g.get_spin(),
            g.get_breathe(),
            g.get_pulse(),
            g.get_flow(),
        )
    }

    /// A key as the window filter handles it: the app first, then the front shell.
    fn key(&self, ev: KeyEvent) -> KeyOutcome {
        let out = self.ctl.borrow_mut().on_key(&ev);
        if out == KeyOutcome::Pty {
            let id = self.front();
            let handle = self.ctl.borrow().handle_of(&id).expect("a handle");
            handle.send_key(&ev);
        }
        self.pump();
        out
    }

    // ---- layout, from the rendered window ---------------------------------------------------

    fn hair(&self) -> u32 {
        self.scale.floor().max(1.0) as u32
    }

    fn strip_left(&self) -> u32 {
        (42.0 * self.scale).round() as u32 + self.hair()
    }

    fn park(&self) {
        self.h.move_to(995.0, 635.0);
        self.h.advance(0);
    }

    fn frame(&self) -> Frame {
        self.park();
        self.h.render()
    }

    /// The end (exclusive, device px) of every full-height divider in the strip row.
    fn edges(&self, f: &Frame) -> Vec<u32> {
        let y = self.h.phys(6.0);
        let hair = self.hair();
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
        self.park();
        let f = self.h.render();
        let e = self.edges(&f);
        assert!(e.len() > i + 1, "no divider after tab {i}: {e:?}");
        ((e[i] + e[i + 1]) as f32 / 2.0 / self.scale, BAR_H / 2.0)
    }

    /// Tab `i`'s left edge and width in device pixels.
    fn span(&self, i: usize) -> (u32, u32) {
        let r = self.row(i);
        (self.strip_left() + r.x as u32, r.w as u32)
    }

    /// The box of tab `i`'s end (the ring or the X), widened, clear of the top rule and the foot.
    fn end_box(&self, i: usize) -> (u32, u32, u32, u32) {
        let (x, w) = self.span(i);
        let s = self.scale;
        (
            ((x + w) as f32 - 10.0 * s - 16.0 * s - 2.0) as u32,
            (4.0 * s) as u32,
            ((x + w) as f32 - 10.0 * s + 2.0) as u32,
            (30.0 * s) as u32,
        )
    }

    /// What is drawn in the end box over its own background (the corner pixel).
    fn end_mark(&self, f: &Frame, i: usize) -> Option<(u32, u32, u32, u32)> {
        let b = self.end_box(i);
        let bg = f.px(b.0, b.1);
        f.bbox_where(b.0, b.1, b.2, b.3, |p| !Frame::near(p, bg, 3))
    }

    /// The bbox of pixels of `c` (within `tol`) in tab `i`'s columns over the strip's height.
    fn band(&self, f: &Frame, i: usize, c: Color, tol: u8) -> Option<(u32, u32, u32, u32)> {
        let (x, w) = self.span(i);
        let want = rgba(c);
        f.bbox_where(x, 0, x + w, self.h.phys(40.0), |p| {
            Frame::near(p, want, tol)
        })
    }

    /// The bbox of any saturated (coloured) pixel at tab `i`'s foot: a mark line of whatever hue.
    fn band_any(&self, f: &Frame, i: usize) -> Option<(u32, u32, u32, u32)> {
        let (x, w) = self.span(i);
        f.bbox_where(
            x,
            self.foot() - self.rows_of_band() - 1,
            x + w,
            self.foot(),
            |p| {
                let hi = p[0].max(p[1]).max(p[2]);
                let lo = p[0].min(p[1]).min(p[2]);
                hi - lo > 40
            },
        )
    }

    /// How many rows of `c` the mark in tab `i` has in its middle column (a run bar's rounded
    /// ends are shorter).
    fn band_rows(&self, f: &Frame, i: usize, c: Color) -> u32 {
        let bb = self.band(f, i, c, 2).expect("a band");
        let x = (bb.0 + bb.2) / 2;
        let want = rgba(c);
        f.count_where(x, 0, x + 1, self.foot(), |p| Frame::near(p, want, 2)) as u32
    }

    /// The strip's foot (exclusive): its height less the hairline under it.
    fn foot(&self) -> u32 {
        self.h.phys(36.0) - self.hair()
    }

    /// Sets the mark clock `ms` after it started (the tests do not step the fake clock first).
    fn mark_clock_at(&self, started: u64, ms: u64) {
        self.clock.set(started + ms);
        self.ctl.borrow().mark_frame_now();
    }

    fn rows_of_band(&self) -> u32 {
        (3.0 * self.scale).round() as u32
    }

    fn hover_tab(&self, i: usize) {
        let (x, y) = self.tab_center(i);
        self.h.move_to(x, y);
        self.h.advance(0);
    }
}

fn ctrl(key: &str, code: &str) -> KeyEvent {
    KeyEvent::new(key, code).ctrl()
}

fn resume_pwsh() -> ShellDef {
    ShellDef {
        id: "pwsh".to_string(),
        name: "PowerShell 7".to_string(),
        exe: "pwsh.exe".to_string(),
        args: ["-NoLogo", "-NoExit", "-Command", "boot"]
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    }
}

fn env_of(m: &FakeState, key: &str) -> Option<String> {
    m.spec()
        .env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

// ---------------------------------------------------------------------------------------------
// indicator (522), attention (609)

#[test]
fn a_working_title_lights_a_background_tab_and_ring_is_the_default() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", &title(T_IDLE));
    f.pump_until("the agent", |f| f.agent_here(1));
    // The poll agrees the agent is there.
    let id = f.ids()[1].clone();
    {
        let mut c = f.ctl.borrow_mut();
        c.hub_mut()
            .queue_poll_answer(&id, true, Some(DetectedAgent::Claude));
        c.hub_mut().drain_poll();
        c.refresh();
    }
    f.say("C:\\b", &title(T_SPIN));
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    f.step(0);
    assert!(f.in_set("working", 1));
    assert_eq!(f.row(1).mark_kind, 1, "no stored word: Ring");
    assert_eq!(f.row(0).mark_state, 0, "none on the front tab");

    let fr = f.frame();
    let accent = f.row(1).mark_color;
    assert!(f.end_mark(&fr, 1).is_some(), "the spinner in the end box");
    let b = f.end_box(0);
    assert_eq!(
        fr.count_where(b.0, b.1, b.2, b.3, |p| Frame::near(p, rgba(accent), 40)),
        0,
        "nothing coloured on the front tab"
    );

    // Minimal: a working run bar in the accent along the foot, no ring.
    f.set_indicator(AgentIndicator::Minimal);
    assert_eq!(f.row(1).mark_kind, 3);
    let t0 = f.clock.get();
    f.mark_clock_at(t0, 600);
    let fr = f.frame();
    assert_eq!(
        f.band_rows(&fr, 1, accent),
        f.rows_of_band(),
        "round(3 * scale) rows"
    );
    assert!(
        f.band(&fr, 0, accent, 2).is_none(),
        "no bar on the front tab"
    );
    assert!(f.end_mark(&fr, 1).is_none(), "no ring under Bar");
}

#[test]
fn a_background_answer_leaves_the_finished_mark_and_visiting_clears_it() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", &title(T_IDLE));
    f.say("C:\\b", &title(T_SPIN));
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    f.say("C:\\b", &title(T_IDLE));
    f.until_step("the check", 100, 5_000, |f| f.row(1).mark_state == 3);
    assert_eq!(f.row(1).mark_kind, 1, "Ring: a check");
    assert!(f.end_mark(&f.frame(), 1).is_some());

    // Bar: a rainbow line.
    f.set_indicator(AgentIndicator::Minimal);
    assert_eq!(f.row(1).mark_kind, 2);
    assert!(f.row(1).mark_rainbow);

    // Visiting it clears the mark.
    f.visit(1);
    f.until_step("the mark to go", 100, 3_000, |f| f.row(1).mark_state == 0);
    assert!(f.band(&f.frame(), 1, f.row(1).mark_color, 2).is_none());
}

#[test]
fn three_working_tabs_each_draw_their_own_bar() {
    let f = Fx::with(Opts::default(), |p| {
        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
    });
    f.tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d"]);
    f.visit(0);
    for c in ["C:\\b", "C:\\c", "C:\\d"] {
        f.say(c, W);
    }
    f.pump_until("three bars", |f| (1..4).all(|i| f.row(i).mark_state == 1));
    let t0 = f.clock.get();
    f.mark_clock_at(t0, 600);
    let fr = f.frame();
    let accent = f.row(1).mark_color;
    let mut feet = Vec::new();
    for i in 1..4 {
        assert_eq!(f.row(i).mark_kind, 3, "tab {i}");
        let (x, w) = f.span(i);
        let bb = f
            .band(&fr, i, accent, 2)
            .unwrap_or_else(|| panic!("no bar on tab {i}"));
        assert!(bb.0 >= x && bb.2 < x + w, "tab {i}'s bar stays in its tab");
        assert_eq!(f.band_rows(&fr, i, accent), f.rows_of_band(), "tab {i}");
        feet.push(bb.3);
    }
    assert!(feet.windows(2).all(|p| p[0] == p[1]), "one foot: {feet:?}");
    assert!(
        f.band(&fr, 0, accent, 2).is_none(),
        "the front tab has none"
    );
}

#[test]
fn a_background_question_is_a_three_pixel_breathing_line_and_a_badge() {
    let f = Fx::with(Opts::default(), |p| {
        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
    });
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", Q);
    f.pump_until("question", |f| f.row(1).mark_state == 2);
    assert_eq!(f.row(1).mark_kind, 2);
    let colour = f.row(1).mark_color;
    f.ctl.borrow().mark_frame_now();
    let fr = f.frame();
    let bb = f
        .band(&fr, 1, colour, 2)
        .expect("the line in the question colour");
    assert_eq!(bb.3 - bb.1 + 1, f.rows_of_band());
    // The mark clock: a second on, the line has breathed down.
    let t0 = f.clock.get();
    f.clock.set(t0 + 1000);
    f.ctl.borrow().mark_frame_now();
    let (_, _, breathe, ..) = f.motion();
    assert!(
        (breathe - mark_phase(1000).breathe).abs() < 1e-4,
        "{breathe}"
    );
    assert!(breathe < 0.5, "faded: {breathe}");
    let faded = f.frame();
    assert!(
        f.band(&faded, 1, colour, 2).is_none(),
        "no pixel in the full question colour once faded"
    );
    // The badge got (1, scale) and the words.
    assert_eq!(
        f.badge.last(),
        Some((Some(16), "1 tab needs a look".to_string()))
    );
}

#[test]
fn opening_does_not_clear_a_question_answering_does() {
    let f = Fx::with(Opts::default(), |p| {
        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
    });
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", Q);
    f.pump_until("question", |f| f.row(1).mark_state == 2);
    f.visit(1);
    f.holds_for("the question after opening", 200, 3_000, |f| {
        f.row(1).mark_state == 2
    });
    assert_eq!(
        f.badge.last().map(|b| b.1),
        Some("1 tab needs a look".to_string()),
        "the badge stays"
    );
    // The answer: a working hook.
    f.say("C:\\b", W);
    f.until_step("the answer", 100, 2_000, |f| !f.in_set("question", 1));
    assert_ne!(f.row(1).mark_state, 2);
    assert_eq!(
        f.badge.last().map(|b| b.0),
        Some(None),
        "the badge is cleared"
    );
}

#[test]
fn finished_is_a_flowing_rainbow_by_default_plain_when_off_and_off_counts_nothing() {
    let f = Fx::with(Opts::default(), |p| {
        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
    });
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", D);
    f.pump_until("finished", |f| f.row(1).mark_state == 3);
    assert_eq!((f.row(1).mark_kind, f.row(1).mark_rainbow), (2, true));
    // The seven colours are in the line.
    let stops: Vec<Color> =
        f.h.comp
            .global::<MarkMotion>()
            .get_rainbow()
            .iter()
            .collect();
    assert_eq!(stops.len(), 7);
    let t0 = f.clock.get();
    f.ctl.borrow().mark_frame_now();
    let fr = f.frame();
    let (x, w) = f.span(1);
    let bb = f.band_any(&fr, 1).expect("the band");
    let y = (bb.1 + bb.3) / 2;
    let line: Vec<[u8; 4]> = (x..x + w).map(|px| fr.px(px, y)).collect();
    for (k, c) in stops.iter().enumerate() {
        let want = rgba(*c);
        assert!(
            line.iter().any(|p| Frame::near(*p, want, 14)),
            "rainbow colour {k} missing from the line"
        );
    }
    // The next clock tick moves them.
    f.clock.set(t0 + 2000);
    f.ctl.borrow().mark_frame_now();
    let fr2 = f.frame();
    let line2: Vec<[u8; 4]> = (x..x + w).map(|px| fr2.px(px, y)).collect();
    assert_ne!(line, line2, "the rainbow flows");

    // Rainbow off: one flat finished colour.
    set_agent_rainbow(&f.prefs, false);
    f.pump_until("flat", |f| !f.row(1).mark_rainbow);
    let flat = f.frame();
    let c = f.row(1).mark_color;
    let bb = f.band(&flat, 1, c, 2).expect("a flat band");
    let y = (bb.1 + bb.3) / 2;
    assert!(
        (x + 2..x + w - 2).all(|px| Frame::near(flat.px(px, y), rgba(c), 2)),
        "one flat colour"
    );

    // Done off: no band, and nothing counted.
    set_agent_done_on(&f.prefs, false);
    f.pump_until("no mark", |f| f.row(1).mark_state == 0);
    assert!(f.band_any(&f.frame(), 1).is_none());
    assert_eq!(
        f.badge.last().map(|b| b.0),
        Some(None),
        "the badge is cleared"
    );
}

/// The marks and the taskbar count follow their Settings rows while Settings stays open (owner bug
/// 2026-10-21, "some settings dont apply instantly"; Prism Terminal applies every row at once).
#[test]
fn the_mark_rows_and_the_taskbar_count_apply_while_settings_is_open() {
    for s in SCALES {
        let f = Fx::with(
            Opts {
                scale: s,
                ..Opts::default()
            },
            |p| {
                LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
            },
        );
        f.tabs(&["C:\\a", "C:\\b"]);
        f.visit(0);
        f.say("C:\\b", Q);
        f.pump_until("question", |f| f.row(1).mark_state == 2);
        assert_eq!(
            f.badge.last().map(|b| b.1),
            Some("1 tab needs a look".to_string()),
            "scale {s}"
        );
        let _settings = f.open_settings();
        // The taskbar count off: the overlay goes at once.
        f.settings_switch("taskbar-badge", false);
        assert_eq!(f.h.comp.get_front_kind(), 1, "scale {s}: Settings stays");
        assert_eq!(
            f.badge.last().map(|b| b.0),
            Some(None),
            "scale {s}: the count left the taskbar with Settings open"
        );
        // And back on: it comes back at once.
        f.settings_switch("taskbar-badge", true);
        assert!(
            f.badge.last().is_some_and(|b| b.0.is_some()),
            "scale {s}: the count is back with Settings open"
        );
        // The question mark off: the tab's mark goes at once.
        f.settings_switch("agent-question-on", false);
        assert_eq!(f.row(1).mark_state, 0, "scale {s}: the mark went");
        assert_eq!(f.h.comp.get_front_kind(), 1, "scale {s}: Settings stays");
    }
}

// ---------------------------------------------------------------------------------------------
// agentHooks (769)

#[test]
fn hooks_light_background_tabs_and_the_state_follows_the_agents_word() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d"]);
    f.visit(0);
    f.say("C:\\b", W);
    f.say("C:\\c", Q);
    f.say("C:\\d", F);
    f.pump_until("three marks", |f| {
        f.row(1).mark_state == 1 && f.row(2).mark_state == 2 && f.row(3).mark_state == 4
    });
    f.say("C:\\b", D);
    f.pump_until("finished", |f| f.row(1).mark_state == 3);
    // Several held at once: Question outranks Failed outranks Finished.
    f.say("C:\\a", W);
    f.say("C:\\c", F);
    f.step(0);
    assert!(f.in_set("question", 2) || f.in_set("failed", 2));
    assert_eq!(
        f.row(2).mark_state,
        if f.in_set("question", 2) { 2 } else { 4 },
        "the strongest held mark is the one shown"
    );
    if f.in_set("question", 2) && f.in_set("failed", 2) {
        assert_eq!(f.row(2).mark_state, 2, "Question outranks Failed");
    }
    assert_eq!(f.row(3).mark_state, 4, "Failed outranks nothing else here");
}

#[test]
fn walking_the_choices_is_not_an_answer() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", &paint(&QUESTION_BOX));
    f.say("C:\\b", Q);
    f.pump_until("question", |f| f.row(1).mark_state == 2);
    f.visit(1);
    for (k, code) in [
        ("ArrowDown", "ArrowDown"),
        ("ArrowUp", "ArrowUp"),
        ("Tab", "Tab"),
    ] {
        assert_eq!(f.key(KeyEvent::new(k, code)), KeyOutcome::Pty);
    }
    f.holds_for("the question across the walk", 250, 6_000, |f| {
        f.in_set("question", 1)
    });
    assert_eq!(f.row(1).mark_state, 2);
}

#[test]
fn a_working_hook_answers_a_question() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", Q);
    f.pump_until("question", |f| f.row(1).mark_state == 2);
    f.say("C:\\b", W);
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    assert!(!f.in_set("question", 1));
}

#[test]
fn a_permission_prompt_raises_question_then_the_approved_work_is_working() {
    // Yes: Enter, the box goes, the spinner title follows.
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    f.say("C:\\a", &title(T_IDLE));
    f.say("C:\\a", &paint(&PERMISSION_BOX));
    f.say("C:\\a", Q);
    f.pump_until("question", |f| f.row(0).mark_state == 2);
    assert_eq!(f.key(KeyEvent::new("Enter", "Enter")), KeyOutcome::Pty);
    f.say("C:\\a", clear());
    f.say("C:\\a", &title(T_SPIN));
    f.until_step("working", 50, 250, |f| f.in_set("working", 0));
    assert!(!f.in_set("question", 0));

    // No: Escape, an idle title; no Working, the question goes once the box is gone.
    let g = Fx::new();
    g.tabs(&["C:\\a"]);
    g.visit(0);
    g.say("C:\\a", &title(T_IDLE));
    g.say("C:\\a", &paint(&PERMISSION_BOX));
    g.say("C:\\a", Q);
    g.pump_until("question", |f| f.row(0).mark_state == 2);
    assert_eq!(g.key(KeyEvent::new("Escape", "Escape")), KeyOutcome::Pty);
    g.say("C:\\a", clear());
    g.say("C:\\a", &title(T_IDLE));
    g.until_step("the question to clear", 250, 6_000, |f| {
        !f.in_set("question", 0)
    });
    assert!(!g.in_set("working", 0), "a refusal is not work");
}

#[test]
fn a_long_tool_with_no_hook_holds_working() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", W);
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    for _ in 0..26 {
        f.say("C:\\b", &title(T_SPIN));
        f.step(960);
        assert!(f.in_set("working", 1), "working all along");
    }
    f.say("C:\\b", &title(T_IDLE));
    f.say("C:\\b", D);
    f.until_step("finished", 100, 3_000, |f| f.row(1).mark_state == 3);
}

#[test]
fn a_subagent_question_stands_until_it_is_answered() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", W);
    f.say("C:\\b", &paint(&PERMISSION_BOX));
    f.say("C:\\b", Q);
    f.pump_until("question", |f| f.row(1).mark_state == 2);
    // A sibling works and the main agent stops while the box is still on the screen.
    f.say("C:\\b", W);
    f.say("C:\\b", D);
    f.say("C:\\b", &paint(&PERMISSION_BOX));
    f.holds_for("the question across the siblings", 250, 3_000, |f| {
        f.in_set("question", 1)
    });
    // The answer: the box goes, and the work goes on.
    f.say("C:\\b", clear());
    f.say("C:\\b", W);
    f.until_step("the answer", 250, 6_000, |f| !f.in_set("question", 1));
}

#[test]
fn a_stop_with_the_spinner_up_stays_working_until_the_star() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", &title(T_IDLE));
    f.say("C:\\b", W);
    f.say("C:\\b", &title(T_SPIN));
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    f.say("C:\\b", D);
    f.step(10);
    assert!(f.in_set("working", 1), "the spinner is still up");
    assert!(!f.in_set("done", 1));
    f.step(40);
    f.say("C:\\b", &title(T_IDLE));
    f.until_step("finished", 20, 300, |f| f.in_set("done", 1));

    // With no star at all within 1500 ms the Stop stands.
    let g = Fx::new();
    g.tabs(&["C:\\a", "C:\\b"]);
    g.visit(0);
    g.say("C:\\b", &title(T_IDLE));
    g.say("C:\\b", W);
    g.say("C:\\b", &title(T_SPIN));
    g.pump_until("working", |f| f.row(1).mark_state == 1);
    g.say("C:\\b", D);
    g.until_step("finished", 100, 1_700, |f| f.in_set("done", 1));
}

#[test]
fn failed_has_a_tooltip_and_the_failed_switch_is_on_by_default() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say("C:\\b", F);
    f.pump_until("failed", |f| f.row(1).mark_state == 4);
    assert_eq!(f.row(1).tip.as_str(), "C:\\b\nFailed: rate limit");
    f.hover_tab(1);
    f.clock.set(f.clock.get() + 1001);
    f.ctl.borrow_mut().tick();
    f.h.advance(0);
    assert!(f.h.comp.get_tip_visible());
    assert_eq!(
        f.h.comp.get_tip_text().as_str(),
        "C:\\b\nFailed: rate limit"
    );

    // The failed switch off: the tab shows Finished instead.
    let g = Fx::with(Opts::default(), |p| set_agent_failed_on(p, false));
    g.tabs(&["C:\\a", "C:\\b"]);
    g.visit(0);
    g.say("C:\\b", F);
    g.pump_until("finished", |f| f.row(1).mark_state == 3);
    assert_eq!(
        g.row(1).tip.as_str(),
        "C:\\b",
        "no failed words once the switch is off"
    );
}

// ---------------------------------------------------------------------------------------------
// backgroundWork (3005), agentDiag (999)

#[test]
fn a_stop_with_one_shell_still_running_stays_working() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.say(
        "C:\\b",
        &paint(&[
            "\u{273b} Churned for 8s \u{b7} done 7:49 \u{b7} 1 shell still running",
            "\u{2500}".repeat(40).as_str(),
            "\u{276f} ",
            "\u{2500}".repeat(40).as_str(),
        ]),
    );
    f.say("C:\\b", W);
    f.say("C:\\b", D);
    f.holds_for("working with a shell still running", 250, 2_000, |f| {
        f.in_set("working", 1) && !f.in_set("done", 1)
    });
    f.say("C:\\b", clear());
    f.until_step("finished", 250, 3_000, |f| f.in_set("done", 1));
    assert!(!f.in_set("working", 1));
}

#[test]
fn the_log_has_the_marks_the_rules_and_no_text() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    let secret_title = "SECRET-TITLE-TEXT";
    f.say(
        "C:\\b",
        &title(&format!("\u{2733} Claude Code {secret_title}")),
    );
    f.say(
        "C:\\b",
        &paint(&["SECRET-SCREEN-ROW", "Do you want to proceed?", " 1. Yes"]),
    );
    f.say("C:\\b", W);
    f.say("C:\\b", Q);
    f.say("C:\\b", D);
    f.until_step("a mark", 100, 2_000, |f| f.row(1).mark_state != 0);
    f.step(6_000);
    let lines = f.diag.lines();
    let kinds: Vec<&str> = lines.iter().map(|l| l.k.as_str()).collect();
    assert!(kinds.contains(&"agent-mark"), "{kinds:?}");
    assert!(kinds.contains(&"agent-hook"), "{kinds:?}");
    let marks: Vec<&flare_diag::agent_diag::Line> =
        lines.iter().filter(|l| l.k == "agent-mark").collect();
    assert!(
        marks.iter().all(|l| l.text("why").is_some()),
        "every mark names its rule"
    );
    let all = format!("{lines:?}");
    for secret in [secret_title, "SECRET-SCREEN-ROW", "Do you want to proceed"] {
        assert!(!all.contains(secret), "{secret} reached the log");
    }
}

// ---------------------------------------------------------------------------------------------
// tabLooks (1840), indicatorStyles (2051)

#[test]
fn the_ring_is_at_the_end_the_close_waits_behind_it_idle_shows_neither() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b", "C:\\c"]);
    f.visit(0);
    f.say("C:\\b", W);
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    let fr = f.frame();
    let ring = f.end_mark(&fr, 1).expect("the ring at the end");
    assert!(f.end_mark(&fr, 2).is_none(), "an idle tab shows neither");
    let (x, w) = f.span(1);
    let s = f.scale;
    let end_right = (x + w) as f32 - 10.0 * s;
    let cx = (ring.0 + ring.2) as f32 / 2.0;
    assert!(
        (cx - (end_right - 8.0 * s)).abs() <= 2.0 * s,
        "centred in the end box"
    );
    // Hover: the ring slides left, the X appears where it was.
    f.hover_tab(1);
    f.h.advance(300);
    let g = f.h.render();
    let b = (
        (end_right - 16.0 * s) as u32,
        (4.0 * s) as u32,
        end_right as u32,
        (30.0 * s) as u32,
    );
    let bright = |p: [u8; 4]| (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 > 90;
    assert!(
        g.count_where(b.0, b.1, b.2, b.3, bright) > 0,
        "the X shows after the ring"
    );
    let region = (
        (end_right - 34.0 * s - 2.0) as u32,
        b.1,
        (end_right - 18.0 * s) as u32,
        b.3,
    );
    let bg = g.px(region.0, region.1);
    let moved = g
        .bbox_where(region.0, region.1, region.2, region.3, |p| {
            !Frame::near(p, bg, 3)
        })
        .expect("the ring is still there, to the left");
    assert!(moved.2 < ring.2, "the ring moved left");
}

#[test]
fn ring_is_alone_on_marked_tabs_in_the_names_ink_bar_is_a_line_per_marked_tab() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d", "C:\\e", "C:\\f"]);
    f.visit(0);
    f.say("C:\\b", W);
    f.say("C:\\c", Q);
    f.say("C:\\d", D);
    f.say("C:\\e", F);
    f.pump_until("four marks", |f| {
        [(1, 1), (2, 2), (3, 3), (4, 4)]
            .iter()
            .all(|(i, s)| f.row(*i).mark_state == *s)
    });
    for i in 1..=4 {
        assert_eq!(f.row(i).mark_kind, 1, "Ring on tab {i}");
    }
    assert_eq!(f.row(5).mark_kind, 0, "idle shows nothing");
    let fr = f.frame();
    for i in 1..=4 {
        assert!(
            f.band_any(&fr, i).is_none(),
            "Ring draws no line on tab {i}"
        );
        assert!(f.end_mark(&fr, i).is_some(), "Ring draws a mark on tab {i}");
    }
    f.set_indicator(AgentIndicator::Minimal);
    let t0 = f.clock.get();
    f.mark_clock_at(t0, 600);
    let fr = f.frame();
    let mut colours = Vec::new();
    for i in 1..=4 {
        assert!(matches!(f.row(i).mark_kind, 2 | 3), "a line on tab {i}");
        assert!(f.band_any(&fr, i).is_some(), "Bar draws a line on tab {i}");
        let c = f.row(i).mark_color;
        colours.push([c.red(), c.green(), c.blue()]);
    }
    colours.sort_unstable();
    colours.dedup();
    assert!(
        colours.len() >= 3,
        "the palette colours differ: {colours:?}"
    );
    assert!(f.band_any(&fr, 5).is_none(), "the idle tab has no line");

    // The front tab's rule per style: working and question show on it, finished and failed do not.
    let g = Fx::with(Opts::default(), |p| {
        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
    });
    g.tabs(&["C:\\a", "C:\\b"]);
    g.visit(0);
    g.say("C:\\a", W);
    g.pump_until("working shows on the front tab", |f| {
        f.row(0).mark_state == 1
    });
    g.say("C:\\a", Q);
    g.pump_until("question shows on the front tab", |f| {
        f.row(0).mark_state == 2
    });
    g.say("C:\\a", W);
    g.say("C:\\a", D);
    g.until_step("finished does not stay on the front tab", 100, 3_000, |f| {
        f.row(0).mark_state == 0
    });
    g.say("C:\\a", W);
    g.say("C:\\a", F);
    g.until_step("failed does not stay on the front tab", 100, 3_000, |f| {
        f.row(0).mark_state == 0
    });
}

#[test]
fn reduced_motion_stills_every_mark_and_minimise_pauses() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    f.ctl.borrow_mut().set_animations_source(Rc::new(|| false));
    f.say("C:\\b", W);
    f.pump_until("working", |f| f.row(1).mark_state == 1);
    assert!(!f.ctl.borrow().mark_clock_running(), "no timer");
    let (still, spin, breathe, pulse, flow) = f.motion();
    assert!(still);
    assert_eq!((spin, breathe, pulse, flow), (0.0, 1.0, 1.0, 0.0));

    // Enabled and minimised: the timer is stopped and the values frozen.
    f.ctl.borrow_mut().set_animations_source(Rc::new(|| true));
    assert!(f.ctl.borrow().mark_clock_running());
    f.clock.set(f.clock.get() + 300);
    f.ctl.borrow().mark_frame_now();
    f.ctl.borrow_mut().on_minimised(true);
    assert!(!f.ctl.borrow().mark_clock_running());
    let frozen = f.motion();
    f.clock.set(f.clock.get() + 5_000);
    f.ctl.borrow().mark_frame_now();
    assert_eq!(f.motion(), frozen, "frozen while minimised");

    // Restored: moving again.
    f.ctl.borrow_mut().on_minimised(false);
    assert!(f.ctl.borrow().mark_clock_running());
    f.clock.set(f.clock.get() + 123);
    f.ctl.borrow().mark_frame_now();
    assert_ne!(f.motion().1, frozen.1, "the spinner turns again");
}

#[test]
fn a_codex_action_required_title_raises_a_question_on_a_background_tab() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    // Codex has no hooks: the poll finds it, its own title says it is asking.
    let id = f.ids()[1].clone();
    {
        let mut c = f.ctl.borrow_mut();
        c.hub_mut()
            .queue_poll_answer(&id, true, Some(DetectedAgent::Codex));
        c.hub_mut().drain_poll();
        c.refresh();
    }
    f.say("C:\\b", &title(T_CODEX_ASKS));
    f.pump_until("a question", |f| f.row(1).mark_state == 2);
    assert_eq!(f.row(1).agent, 2, "the Codex icon");
    assert_eq!(
        f.badge.last().map(|b| b.1),
        Some("1 tab needs a look".to_string())
    );
}

// ---------------------------------------------------------------------------------------------
// launchSkeleton (3279), restore (3378)

fn restoring_fx() -> (Fx, Arc<FixedSessions>) {
    let lookup = FixedSessions::new(&[SESSION_A]);
    let l: Arc<dyn SessionLookup> = lookup.clone();
    let f = Fx::with(
        Opts {
            saved: vec![
                ("C:\\c", Some(AgentKind::Claude)),
                ("C:\\x", Some(AgentKind::Codex)),
                ("C:\\p", None),
            ],
            lookup: Some(l),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        },
        |_| {},
    );
    (f, lookup)
}

fn restore(f: &Fx) {
    AppCtl::start_restore(&f.ctl);
    f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
    f.pump_until("the spawns", |f| f.sp.made().len() >= 3);
}

fn grid_text(f: &Fx, id: &str) -> String {
    let handle = f.ctl.borrow().handle_of(id).expect("a handle");
    let grid = handle.snapshot();
    (0..grid.rows())
        .flat_map(|r| (0..grid.cols()).map(move |c| (c, r)))
        .map(|(c, r)| grid.cell(c, r).ch)
        .collect()
}

#[test]
fn the_first_frame_has_the_tabs_the_skeleton_and_the_ring() {
    let (f, _lookup) = restoring_fx();
    // Before anything has answered: both agent rows are there, both ringed; the front has the skeleton.
    assert_eq!(f.rows(), 3);
    assert!(f.row(0).loading && f.row(1).loading);
    assert!(!f.row(2).loading, "a plain shell is not restoring");
    assert!(f.h.comp.get_skeleton_shown());
    assert!(!f.h.comp.get_skeleton_leaving());
    assert_eq!((f.row(0).agent, f.row(1).agent), (1, 2));

    restore(&f);
    assert_eq!(
        env_of(&f.shell("C:\\c"), "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_A),
        "the shell is spawned with the resume"
    );
    let id = f.ids()[0].clone();
    // The shell's own text first, then the agent paints. The skeleton covers the grid until the agent
    // has painted and fades 180 ms later; the shell's prompt is never in the grid once it is gone.
    f.say("C:\\c", "PS C:\\c> claude --resume\r\n");
    let mut shown_gone_with_prompt = false;
    for _ in 0..4 {
        f.step(16);
        if !f.h.comp.get_skeleton_shown() && grid_text(&f, &id).contains("PS C:") {
            shown_gone_with_prompt = true;
        }
    }
    f.say("C:\\c", &title("claude"));
    f.say("C:\\c", &title(T_IDLE));
    // The reveal waits on real time (150 ms after the agent paints), not on the fake clock.
    f.tick_until("the fade to start", |f| f.h.comp.get_skeleton_leaving());
    assert!(f.h.comp.get_skeleton_shown(), "still there while it fades");
    assert!(
        !grid_text(&f, &id).contains("PS C:"),
        "the clear left no shell text"
    );
    f.clock.set(f.clock.get() + 181);
    f.tick_until("the skeleton to go", |f| !f.h.comp.get_skeleton_shown());
    assert!(!f.h.comp.get_skeleton_leaving());
    assert!(
        !shown_gone_with_prompt,
        "the prompt was visible at some frame"
    );
}

#[test]
fn a_tab_behind_shows_its_agent_at_once_and_restored_tabs_carry_no_marks() {
    let (f, _lookup) = restoring_fx();
    assert_eq!(f.row(1).agent, 2, "the Codex mark from the first frame");
    assert!(f.row(1).loading);
    restore(&f);
    assert_eq!(f.row(1).agent, 2);
    f.ctl.borrow_mut().hub_mut().set_looking(None, false);
    f.say("C:\\c", &title(T_IDLE));
    f.say("C:\\x", &title(T_CODEX_ASKS));
    f.say("C:\\x", &title("x"));
    f.step(3_000);
    for i in 0..3 {
        for set in ["done", "question", "failed", "working"] {
            assert!(!f.in_set(set, i), "{set} on restored tab {i}");
        }
        assert_eq!(f.row(i).mark_state, 0, "no mark on restored tab {i}");
    }
    assert_eq!(
        f.badge.last().map(|b| b.0).unwrap_or(None),
        None,
        "the badge is clear"
    );
}

#[test]
fn a_saved_claude_and_a_plain_tab_come_back_the_right_way() {
    let (f, lookup) = restoring_fx();
    restore(&f);
    // The restore returns as the answers arrive: wait for all three shells and the lookup.
    f.pump_until("all three shells and the lookup", |f| {
        let made = f.sp.made();
        ["C:\\c", "C:\\x", "C:\\p"]
            .iter()
            .all(|c| made.iter().any(|m| m.spec().cwd == *c))
            && !lookup.asked().is_empty()
    });
    assert_eq!(
        f.cwds(),
        ["C:\\c", "C:\\x", "C:\\p"],
        "strip order and folders kept"
    );
    assert_eq!(lookup.asked(), ["C:\\c"], "only the Claude folder is asked");
    assert_eq!(
        env_of(&f.shell("C:\\c"), "FLARE_RESUME_ID").as_deref(),
        Some(SESSION_A)
    );
    assert!(env_of(&f.shell("C:\\p"), "FLARE_RESUME_ID").is_none());
    assert_eq!(f.shell("C:\\p").spec().args, resume_pwsh().args);
    assert!(
        f.shell("C:\\x")
            .spec()
            .args
            .last()
            .is_some_and(|a| a.ends_with("codex resume --last"))
    );
}

// ---------------------------------------------------------------------------------------------
// closeAsk (3482), reviewKeys (4458), agentKeys (3057)

#[test]
fn an_idle_agent_still_asks_naming_claude_a_working_one_says_working_for() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    f.say("C:\\a", &title(T_IDLE));
    f.pump_until("the agent", |f| f.agent_here(0));
    assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(
        f.h.comp.get_dialog_title().as_str(),
        "Close the tab and end the agent?"
    );
    let (_, body, _) = f.ctl.borrow().ask_text().expect("an ask");
    assert!(body.contains("Claude"), "{body}");
    // Tab and Shift+Tab stay in the two buttons.
    f.h.advance(0);
    assert!(f.h.comp.get_chrome_has_focus());
    f.h.key_text("\t");
    f.h.advance(0);
    assert!(f.h.comp.get_dialog_open() && f.h.comp.get_chrome_has_focus());
    f.h.key_with(true, false, "\t");
    f.h.advance(0);
    assert!(f.h.comp.get_dialog_open() && f.h.comp.get_chrome_has_focus());
    f.h.comp.invoke_dialog_cancel();
    assert!(!f.h.comp.get_dialog_open());
    assert_eq!(f.rows(), 1, "Cancel keeps the tab");

    // Working for 125 s.
    f.say("C:\\a", W);
    f.pump_until("working", |f| f.working(0));
    f.clock.set(f.clock.get() + 125_000);
    assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert_eq!(
        f.h.comp.get_dialog_title().as_str(),
        "Stop the agent and close?"
    );
    let (_, body, _) = f.ctl.borrow().ask_text().expect("an ask");
    assert!(body.contains("2 minutes"), "{body}");
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(f.rows(), 0, "confirm closes");
    f.pump_until("the shell to be killed", |f| f.shell("C:\\a").killed());
}

#[test]
fn closing_an_agents_tab_asks() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    f.say("C:\\a", &title(T_IDLE));
    f.pump_until("the agent", |f| f.agent_here(0));
    let before = f.shell("C:\\a").written();
    assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
    assert!(f.h.comp.get_dialog_open(), "Ctrl+W opens the dialog");
    f.step(50);
    assert_eq!(f.shell("C:\\a").written(), before, "the shell got no byte");
    assert_eq!(f.rows(), 1);
}

#[test]
fn agent_hotkeys_and_the_guard_follow_their_switches() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    let a = f.ids()[0].clone();
    // Ctrl+PgDn switches tabs by default.
    assert_eq!(f.key(ctrl("PageDown", "PageDown")), KeyOutcome::Consumed);
    assert_eq!(f.front(), f.ids()[1]);
    f.visit(0);
    // With the hotkeys off it reaches the shell.
    set_agent_keys(&f.prefs, false);
    assert_eq!(f.key(ctrl("PageDown", "PageDown")), KeyOutcome::Pty);
    assert_eq!(f.front(), a, "no switch");
    f.pump_until("the key at the shell", |f| {
        !f.shell("C:\\a").written().is_empty()
    });
    set_agent_keys(&f.prefs, true);

    // The guard: a plain shell gets both.
    let d = || ctrl("d", "KeyD");
    let c = || ctrl("c", "KeyC");
    assert_eq!(f.key(d()), KeyOutcome::Pty);
    assert_eq!(f.key(c()), KeyOutcome::Pty);
    // An idle agent: Ctrl+D never reaches it, Ctrl+C is taken.
    f.say("C:\\a", &title(T_IDLE));
    f.pump_until("the agent", |f| f.agent_here(0));
    assert_eq!(f.key(d()), KeyOutcome::Consumed);
    assert_eq!(f.key(c()), KeyOutcome::Consumed);
    // Working: Ctrl+C passes.
    f.say("C:\\a", W);
    f.pump_until("working", |f| f.working(0));
    assert_eq!(f.key(c()), KeyOutcome::Pty);
    // The guard off: both pass.
    set_agent_guard(&f.prefs, false);
    f.say("C:\\a", D);
    f.step(100);
    assert_eq!(f.key(d()), KeyOutcome::Pty);
}

// ---------------------------------------------------------------------------------------------
// sonaraQuestion (3756), imagePaste (4016)

#[test]
fn sonara_hears_the_shown_question_with_the_tab_token() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    let token = env_of(&f.shell("C:\\a"), "FLARE_TAB_ID").expect("the shell has a tab id");
    // Only a Claude tab is believed (audit L22).
    f.say("C:\\a", &title(T_IDLE));
    f.pump_until("the agent", |f| f.agent_here(0));
    f.say("C:\\a", &paint_owned(&screens("run2-01-question-1")));
    f.step(150);
    f.step(0);
    assert_eq!(
        f.posts.lock().unwrap().clone(),
        vec![(token.clone(), ShownQuestion::Text("Pick a colour?".into()))]
    );
    // A redraw of the same screen: nothing more.
    f.say("C:\\a", &paint_owned(&screens("run2-01-question-1")));
    f.step(300);
    assert_eq!(f.posts.lock().unwrap().len(), 1);
    // The next question, then the review.
    f.say("C:\\a", &paint_owned(&screens("run2-02-right")));
    f.step(300);
    f.say("C:\\a", &paint_owned(&screens("run2-08-enter-answer")));
    f.step(300);
    assert_eq!(
        f.posts.lock().unwrap().clone(),
        vec![
            (token.clone(), ShownQuestion::Text("Pick a colour?".into())),
            (token.clone(), ShownQuestion::Text("Pick a size?".into())),
            (token, ShownQuestion::Review),
        ]
    );
}

#[test]
fn the_image_key_and_shift_enter_follow_the_armed_agent() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    let id = f.ids()[0].clone();
    let shell = f.shell("C:\\a");
    let wait_for = |want: &[u8]| {
        f.pump_until("the bytes", |_| shell.written().ends_with(want));
    };
    // A plain shell: the image key is 0x16 and Shift+Enter is an ordinary Enter.
    f.clip.set_image(true);
    assert_eq!(f.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
    wait_for(&[0x16]);
    assert_eq!(
        f.key(KeyEvent::new("Enter", "Enter").shift()),
        KeyOutcome::Pty
    );
    wait_for(b"\r");
    // A bare braille spinner arms nothing.
    f.say("C:\\a", &title("\u{280b} working"));
    f.step(1_100);
    assert_eq!(f.ctl.borrow().armed_agent(&id), None);
    // The title arms within a second, before any poll.
    f.say("C:\\a", &title(T_IDLE));
    f.step(1_000);
    assert_eq!(f.ctl.borrow().armed_agent(&id), Some(AgentKind::Claude));
    assert_eq!(
        f.key(KeyEvent::new("Enter", "Enter").shift()),
        KeyOutcome::Consumed
    );
    wait_for(b"\n");
    assert_eq!(f.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
    wait_for(b"\x1bv");
}

// ---------------------------------------------------------------------------------------------
// scaling (roadmap)

#[test]
fn marks_work_at_every_scale() {
    for s in SCALES {
        for bar in [false, true] {
            let f = Fx::with(
                Opts {
                    scale: s,
                    ..Opts::default()
                },
                |p| {
                    set_agent_rainbow(p, false);
                    if bar {
                        LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
                    }
                },
            );
            f.tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d", "C:\\e"]);
            f.visit(0);
            f.say("C:\\b", W);
            f.say("C:\\c", Q);
            f.say("C:\\d", D);
            f.say("C:\\e", F);
            f.pump_until("four marks", |f| {
                (1..=4).all(|i| f.row(i).mark_state == i as i32)
            });
            let t0 = f.clock.get();
            // The run bar is mid-lap at 600 ms; the question line is at full breath at 0.
            f.mark_clock_at(t0, 600);
            let at_600 = f.frame();
            f.mark_clock_at(t0, 0);
            let at_0 = f.frame();
            let hair = f.hair();
            for i in 1..=4 {
                let fr = if i == 1 { &at_600 } else { &at_0 };
                let (x, w) = f.span(i);
                let c = f.row(i).mark_color;
                if bar {
                    let bb = f
                        .band(fr, i, c, 2)
                        .unwrap_or_else(|| panic!("no line on tab {i} at {s}"));
                    assert_eq!(
                        f.band_rows(fr, i, c),
                        f.rows_of_band(),
                        "tab {i} at {s}: round(3 * scale) rows"
                    );
                    assert!(bb.0 >= x && bb.2 < x + w, "tab {i} at {s}: inside its tab");
                } else {
                    let ring = f
                        .end_mark(fr, i)
                        .unwrap_or_else(|| panic!("no ring on tab {i} at {s}"));
                    if i == 1 {
                        let size = 12.0 * s;
                        assert!(
                            ((ring.2 - ring.0 + 1) as f32 - size).abs() <= 1.0
                                && ((ring.3 - ring.1 + 1) as f32 - size).abs() <= 1.0,
                            "the spinner is 12 css px at {s}: {ring:?}"
                        );
                    }
                    assert!(ring.1 > hair, "tab {i} at {s}: clear of the top rule");
                    assert!(
                        ring.2 + hair < x + w,
                        "tab {i} at {s}: clear of the divider"
                    );
                    let cy = (ring.1 + ring.3) as f32 / 2.0;
                    let want = (f.foot() as f32) / 2.0;
                    assert!(
                        (cy - want).abs() <= 1.5,
                        "tab {i} at {s}: centred, {cy} vs {want}"
                    );
                }
            }
        }
    }
}
