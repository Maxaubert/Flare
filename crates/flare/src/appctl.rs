//! AppCtl: every callback, the restore, the close and the timers (Task 30).
//!
//! Source: Prism Terminal's `App.tsx` (every effect becomes a method) and `index.ts` (hand-off,
//! restore, close). One `AppCtl` lives on the UI thread behind an `Rc<RefCell<..>>`; Slint callbacks
//! reach it through a `Weak` and `try_borrow_mut` (a re-entrant call is dropped, never a panic).
//! Nothing here blocks (rule 34): stats, the argument walk, the session lookup and the folder
//! picker run on worker threads and answer with a [`UiMsg`] through [`Deps::post`].
//!
//! Left to Task 30b (the window): the `grid-resized`, `minimize-clicked`, `maximize-clicked`,
//! `close-clicked` and `toggle-grid-renderer` callbacks (they need the `GridHost` and the winit
//! window), which call [`AppCtl::on_grid_dims`] and [`AppCtl::on_close_requested`]; the winit
//! filter that feeds [`AppCtl::on_key`], [`AppCtl::on_cursor`] and the drop events; the hand-off
//! server; the cursor-at-the-edge watch for a collapsed sidebar.
#![allow(dead_code)] // the window (Task 30b) is the caller

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use flare_agent::close::holds_window_close;
use flare_agent::kinds::{AgentKind, CODEX_RESUME, DetectedAgent};
use flare_settings::kv::PrefStore;
use flare_settings::new_tab::{NewTabMode, new_tab_folder, new_tab_mode};
use flare_settings::recent_roots::{pinned_roots, recent_roots};
use flare_settings::sidebar::{SidebarMode, sidebar_mode};
use flare_settings::tab_layout::{TabLayout, tab_layout};
use flare_settings::tab_switch::tab_switch;
use flare_settings::term_prefs::agent_keys;
use flare_tabs::close_flow::{CloseFlow, CloseVerdict, Confirmed};
use flare_tabs::drag_flight::DragInFlight;
use flare_tabs::keymap::{ChordId, KeyContext, key_label};
use flare_tabs::model::TabModel;
use flare_tabs::places::place_rows;
use flare_tabs::restore::{
    Restored, SavedTabs, SessionLookup, is_unsafe_restore_folder, resolve_restore,
    with_launch_folders,
};
use flare_tabs::reveal::Point;
use flare_tabs::tab_drag::Release;
use flare_tabs::tabs::TabKind;
use flare_tabs::tabs_store::TabsStore;
use flare_tabs::tip_place::{BoxF, TipSide};
use flare_term::handle::TermHandle;
use flare_term::keys::KeyEvent;
use flare_win::argv::{folders_from_argv, own_files};
use flare_win::single_instance::Handoff;
use flare_win::window_state::{NormalBounds, Observed, WindowStateStore};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::chords::{self, ChordTarget, Dispatch};
use crate::menus_ui::{
    MenuKind, MenuRowData, OverflowRowData, TipHost, glyph_path, overflow_rows, plus_menu_rows,
    tab_menu_rows, toggle_pin_in_menu,
};
use crate::sidebar_ui::{SidebarCtl, SidebarEvent};
use crate::startup::{Ask, PendingFolders, ask_view, end_of_life, tab_ask, window_ask};
use crate::tab_host::{HostEvent, TabHost};
use crate::tabs_ui::{
    GeomData, MarkRole, SideDrag, StripDrag, View, ViewInput, build_view, slot_to_model, sync_rows,
};
use crate::{AppWindow, MenuRow, OverflowRow, Place, StripGeom, TabRow};

/// Everything a worker thread can say to the UI thread. WHY a message and not
/// `slint::invoke_from_event_loop` closures: the headless platform has no event loop proxy, and an
/// `Rc<RefCell<AppCtl>>` is not `Send`. Production posts with `invoke_from_event_loop` into a
/// thread-local that holds the `AppCtl` and calls `on_msg` (Task 30b); tests post into a queue and
/// call `pump()`.
#[derive(Debug)]
pub enum UiMsg {
    RestoreDone(Restored),
    Picked(Option<PathBuf>),
    /// A dropped path, resolved by a worker to the folder it names (or holds).
    FolderResolved(String),
    /// The folders a hand-off's arguments named, resolved by a worker.
    Folders(Vec<String>),
    Handoff(Handoff),
    HostWake,
}

pub type Post = Arc<dyn Fn(UiMsg) + Send + Sync>;
type PickAnswer = Arc<dyn Fn(Option<PathBuf>) + Send + Sync>;

pub struct Deps {
    pub prefs: PrefStore,
    pub tabs_store: Arc<TabsStore>,
    pub window_store: Arc<WindowStateStore>,
    pub host: TabHost,
    pub lookup: Arc<dyn SessionLookup>,
    pub is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub home: String,
    pub clock: Rc<dyn Fn() -> u64>,
    /// Worker threads answer the UI thread with a `UiMsg`.
    pub post: Post,
    /// Production: `flare_win::pick::pick_folder` parented to the window; tests: a fake that
    /// answers at once.
    pub pick_folder: Rc<dyn Fn(Option<PathBuf>, PickAnswer)>,
    pub clipboard: Rc<dyn Fn(&str)>,
    pub open_url: Rc<dyn Fn(&str)>,
    /// Production: `GridHost::set_source(Box::new(LiveSource::new(h)))`; tests: records the id.
    pub set_grid_source: Rc<dyn Fn(Option<Arc<TermHandle>>)>,
    pub raise: Rc<dyn Fn()>,
    pub quit: Rc<dyn Fn()>,
    pub hide: Rc<dyn Fn()>,
    pub launch_folders: Vec<String>,
    pub e2e: bool,
}

/// What the window does with a key after `on_key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The app took it.
    Consumed,
    /// The front shell gets it.
    Pty,
    /// A Slint control (the find box, the dialog) gets it.
    Slint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    KeepOpen,
    Close,
}

struct MenuState {
    kind: MenuKind,
    rows: Vec<(MenuRowData, String)>,
}

struct OverflowState {
    all: bool,
    query: String,
}

/// The window's height of the bar in css pixels (`Theme.bar-height`).
const BAR_H: f32 = 36.0;
/// Where the +N list hangs when a chord opened it: from the right, clear of the window buttons.
const CHORD_LIST_RIGHT_GAP: f32 = 170.0;
/// The timer period while anything is pending.
const TICK_MS: u64 = 50;

pub struct AppCtl {
    weak: slint::Weak<AppWindow>,
    me: Weak<RefCell<AppCtl>>,
    deps: Deps,
    prefs: PrefStore,
    model: TabModel,
    sidebar: SidebarCtl,
    tips: TipHost,
    strip_drag: StripDrag,
    side_drag: SideDrag,
    /// The tab a press landed on (a click that never travels picks it).
    pressed: Option<String>,
    flight: DragInFlight,
    close_flow: CloseFlow,
    ask: Option<Ask>,
    pending: PendingFolders,
    menu: Option<MenuState>,
    overflow: Option<OverflowState>,
    bounds: NormalBounds,
    maximised: bool,
    grid_dims: (usize, usize),
    scale: f64,
    timer: slint::Timer,
    timer_on: bool,
    rows_model: Rc<VecModel<TabRow>>,
    view: View,
    last_geom: Option<GeomData>,
    query: String,
    boot_saved: SavedTabs,
    restore_started: bool,
    source_id: Option<String>,
    last_saved: Option<SavedTabs>,
    /// Agents that are mid-answer: tab id to the clock time they started (M6 fills this).
    working: HashMap<String, u64>,
    ended: bool,
}

/// Binds one callback of the window to a method of the controller.
macro_rules! bind {
    ($app:expr, $rc:expr, $reg:ident, |$c:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
        let weak = Rc::downgrade($rc);
        $app.$reg(move |$($a: $t),*| {
            if let Some(rc) = weak.upgrade() {
                if let Ok(mut $c) = rc.try_borrow_mut() {
                    $body;
                }
            }
        });
    }};
}

fn role_colour(role: MarkRole) -> slint::Color {
    match role {
        MarkRole::Working => slint::Color::from_rgb_u8(0xd9, 0xa4, 0x41),
        MarkRole::Done => slint::Color::from_rgb_u8(0x6f, 0xb3, 0x6f),
        MarkRole::Question => slint::Color::from_rgb_u8(0x5b, 0x9b, 0xd5),
        MarkRole::Failed => slint::Color::from_rgb_u8(0xd9, 0x5b, 0x5b),
    }
}

fn mark_colour(role: Option<MarkRole>) -> slint::Color {
    role.map_or(slint::Color::from_argb_u8(0, 0, 0, 0), role_colour)
}

fn menu_row(r: &MenuRowData) -> MenuRow {
    MenuRow {
        label: r.label.as_str().into(),
        hint: r.hint.as_str().into(),
        icon: glyph_path(r.icon).into(),
        filled_icon: r.filled_icon,
        disabled: r.disabled,
        trailing_icon: r.trailing_icon.map_or("", glyph_path).into(),
        trailing_filled: r.trailing_filled,
        trailing_tip: r.trailing_tip.as_str().into(),
    }
}

fn overflow_row(r: &OverflowRowData) -> OverflowRow {
    OverflowRow {
        id: r.id.as_str().into(),
        label: r.label.as_str().into(),
        folder: r.folder.as_str().into(),
        needs: r.needs,
        loading: r.loading,
        settings: r.settings,
    }
}

fn detected(kind: Option<AgentKind>) -> DetectedAgent {
    match kind {
        Some(AgentKind::Claude) => DetectedAgent::Claude,
        Some(AgentKind::Codex) => DetectedAgent::Codex,
        None => DetectedAgent::Other,
    }
}

impl AppCtl {
    /// Binds EVERY callback of the tab window's contract, boots the model from `tabs_store.load()`
    /// (sync-ok: startup), draws the placeholders, and returns. The restore starts with
    /// [`AppCtl::start_restore`].
    pub fn new(app: &AppWindow, deps: Deps) -> Rc<RefCell<AppCtl>> {
        let prefs = deps.prefs.clone();
        let now = (deps.clock)();
        let boot_saved = deps.tabs_store.load(); // sync-ok: startup read of tabs.json
        let model = TabModel::boot(&boot_saved, now);
        let rows_model: Rc<VecModel<TabRow>> = Rc::new(VecModel::default());
        app.set_tab_rows(ModelRc::from(rows_model.clone()));
        let ctl = AppCtl {
            weak: app.as_weak(),
            me: Weak::new(),
            sidebar: SidebarCtl::new(&prefs, now),
            prefs,
            deps,
            model,
            tips: TipHost::new(),
            strip_drag: StripDrag::new(),
            side_drag: SideDrag::new(),
            pressed: None,
            flight: DragInFlight::new(),
            close_flow: CloseFlow::new(),
            ask: None,
            pending: PendingFolders::new(),
            menu: None,
            overflow: None,
            bounds: NormalBounds::new(),
            maximised: false,
            grid_dims: (80, 24),
            scale: 1.0,
            timer: slint::Timer::default(),
            timer_on: false,
            rows_model,
            view: View {
                rows: Vec::new(),
                geom: GeomData {
                    more_x: 0,
                    more_w: 0,
                    plus_x: 0,
                    plus_w: 0,
                    more_count: 0,
                    dots: Vec::new(),
                },
                has_tabs: false,
                front_kind: 3,
            },
            last_geom: None,
            query: String::new(),
            boot_saved,
            restore_started: false,
            source_id: None,
            last_saved: None,
            working: HashMap::new(),
            ended: false,
        };
        let rc = Rc::new(RefCell::new(ctl));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        Self::bind_callbacks(app, &rc);
        rc.borrow_mut().refresh();
        rc
    }

    fn bind_callbacks(app: &AppWindow, rc: &Rc<RefCell<AppCtl>>) {
        bind!(
            app,
            rc,
            on_strip_press,
            |c, id: SharedString, i: i32, x: f32, origin: f32| {
                c.strip_press(&id, i, f64::from(x), f64::from(origin))
            }
        );
        bind!(app, rc, on_strip_moved, |c, x: f32| c
            .strip_moved(f64::from(x)));
        bind!(app, rc, on_strip_released, |c| c.strip_released());
        bind!(
            app,
            rc,
            on_side_press,
            |c, id: SharedString, i: i32, y: f32, first: f32, top: f32, bottom: f32| {
                c.side_press(&id, i, [y, first, top, bottom].map(f64::from))
            }
        );
        bind!(app, rc, on_side_moved, |c, y: f32| c
            .side_moved(f64::from(y)));
        bind!(app, rc, on_side_released, |c| c.side_released());
        bind!(app, rc, on_tab_closed, |c, id: SharedString| c
            .request_close(&id));
        bind!(
            app,
            rc,
            on_tab_menu,
            |c, id: SharedString, x: f32, y: f32| { c.open_tab_menu(&id, x, y) }
        );
        bind!(app, rc, on_new_tab, |c| c.new_tab());
        bind!(app, rc, on_plus_menu, |c, x: f32, y: f32| c
            .open_plus_menu(x, y));
        bind!(app, rc, on_more_clicked, |c, x: f32, y: f32| c
            .open_overflow(false, x, y));
        bind!(app, rc, on_settings_clicked, |c| c.toggle_settings());
        bind!(app, rc, on_strip_resized, |c, _w: i32| c.refresh());
        bind!(app, rc, on_side_find_edited, |c, q: SharedString| c
            .find_edited(&q));
        bind!(app, rc, on_side_find_accepted, |c| c.find_accepted());
        bind!(app, rc, on_side_find_escape, |c| c.find_escape());
        bind!(app, rc, on_grip_moved, |c, w: f32| c
            .grip_moved(f64::from(w)));
        bind!(app, rc, on_grip_released, |c, w: f32| c
            .grip_released(f64::from(w)));
        bind!(app, rc, on_place_picked, |c, p: SharedString| c
            .open_recent(&p));
        bind!(app, rc, on_start_new, |c| c.new_tab());
        bind!(app, rc, on_start_settings, |c| c.toggle_settings());
        bind!(app, rc, on_start_github, |c| c.open_github());
        bind!(app, rc, on_menu_picked, |c, i: i32| c.menu_picked(i));
        bind!(app, rc, on_menu_trailing, |c, i: i32| c.menu_trailing(i));
        bind!(app, rc, on_menu_dismissed, |c| c.close_menu());
        bind!(app, rc, on_overflow_picked, |c, id: SharedString| c
            .overflow_picked(&id));
        bind!(
            app,
            rc,
            on_overflow_row_menu,
            |c, id: SharedString, x: f32, y: f32| { c.overflow_row_menu(&id, x, y) }
        );
        bind!(app, rc, on_overflow_dismissed, |c| c.close_overflow());
        bind!(app, rc, on_overflow_accepted, |c| c.overflow_accepted());
        bind!(app, rc, on_overflow_query_edited, |c, q: SharedString| c
            .overflow_query(&q));
        bind!(app, rc, on_dialog_cancel, |c| c.dialog_cancel());
        bind!(app, rc, on_dialog_confirm, |c| c.dialog_confirm());
        bind!(
            app,
            rc,
            on_tip_request,
            |c, t: SharedString, x: f32, y: f32, w: f32, h: f32| c.tip_request(&t, x, y, w, h)
        );
        bind!(app, rc, on_bar_double_clicked, |c| c.toggle_maximise());
        bind!(app, rc, on_title_pressed, |c| c.title_pressed());
    }

    // ---- small helpers ------------------------------------------------------------------------

    fn now(&self) -> u64 {
        (self.deps.clock)()
    }

    fn app(&self) -> Option<AppWindow> {
        self.weak.upgrade()
    }

    pub fn model(&self) -> &TabModel {
        &self.model
    }

    /// M6's hook (and the tests'): the model, to attach an agent to a tab.
    pub fn model_mut(&mut self) -> &mut TabModel {
        &mut self.model
    }

    /// M6's hook: an agent is mid-answer since `since_ms` (None: it stopped).
    pub fn set_working(&mut self, id: &str, since_ms: Option<u64>) {
        match since_ms {
            Some(t) => {
                self.working.insert(id.to_string(), t);
            }
            None => {
                self.working.remove(id);
            }
        }
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn timer_running(&self) -> bool {
        self.timer_on
    }

    fn vertical(&self) -> bool {
        tab_layout(&self.prefs) == TabLayout::Vertical
    }

    fn front_id(&self) -> Option<String> {
        self.model.state.active_id.clone()
    }

    fn front_handle(&self) -> Option<Arc<TermHandle>> {
        let t = self.model.active_shell()?;
        self.deps.host.handle(&t.id)
    }

    fn focus_terminal(&self) {
        if let Some(app) = self.app() {
            app.invoke_focus_terminal();
        }
    }

    fn key_label_of(&self, id: ChordId) -> &'static str {
        key_label(id, agent_keys(&self.prefs))
    }

    // ---- the view -------------------------------------------------------------------------------

    /// build_view, then push every property; a changed tab report goes to `tabs_store`.
    pub fn refresh(&mut self) {
        let Some(app) = self.app() else { return };
        let now = self.now();
        let scale = f64::from(app.get_scale());
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let strip_phys = app.get_strip_phys_width();
        let none: HashMap<String, f32> = HashMap::new();
        let needs: HashMap<String, MarkRole> = HashMap::new();
        let base = build_view(&ViewInput {
            model: &self.model,
            home: &self.deps.home,
            query: &self.query,
            strip_phys,
            scale: self.scale,
            needs: &needs,
            offsets: &none,
            carried: None,
        });
        let mut offsets = HashMap::new();
        if self.strip_drag.carrying() {
            offsets = self.strip_drag.offsets(&base);
        } else if self.side_drag.carrying() {
            let ids: Vec<String> = base.rows.iter().map(|r| r.id.clone()).collect();
            offsets = self.side_drag.offsets(&ids);
        }
        let carried = self
            .strip_drag
            .carried()
            .or_else(|| self.side_drag.carried());
        let mut view = base;
        for r in &mut view.rows {
            r.offset_css = offsets.get(&r.id).copied().unwrap_or(0.0);
            r.carried = carried.as_deref() == Some(r.id.as_str());
        }

        sync_rows(&self.rows_model, &view.rows, &mark_colour);
        if self.last_geom.as_ref() != Some(&view.geom) {
            let dots: Vec<slint::Color> = view.geom.dots.iter().copied().map(role_colour).collect();
            app.set_strip_geom(StripGeom {
                more_x: view.geom.more_x,
                more_w: view.geom.more_w,
                plus_x: view.geom.plus_x,
                plus_w: view.geom.plus_w,
                more_count: view.geom.more_count,
                dots: ModelRc::from(Rc::new(VecModel::from(dots))),
            });
            self.last_geom = Some(view.geom.clone());
        }
        app.set_has_tabs(view.has_tabs);
        app.set_front_kind(view.front_kind);
        app.set_layout_vertical(self.vertical());
        app.set_carrying(self.strip_drag.carrying() || self.side_drag.carrying());
        app.set_drag_in_flight(self.flight.on());

        // The static texts, from the chord table (they follow the agent-keys preference).
        let new_key = self.key_label_of(ChordId::NewTab);
        app.set_new_key(new_key.into());
        app.set_settings_key(self.key_label_of(ChordId::Settings).into());
        app.set_find_tab_key(self.key_label_of(ChordId::FindTab).into());
        app.set_new_tab_title(
            format!("New tab ({new_key}). Right-click for recent folders").into(),
        );
        app.set_version_text(format!("v{}", env!("CARGO_PKG_VERSION")).into());

        if view.front_kind == 2 {
            let places: Vec<Place> =
                place_rows(&pinned_roots(&self.prefs), &recent_roots(&self.prefs))
                    .into_iter()
                    .map(|p| Place {
                        path: p.path.into(),
                        name: p.name.into(),
                        near: p.near.into(),
                        label: p.label.into(),
                        pinned: p.pinned,
                    })
                    .collect();
            app.set_places(ModelRc::from(Rc::new(VecModel::from(places))));
        }

        self.sidebar
            .hold(app.get_chrome_has_focus(), self.menu.is_some(), now);
        let sv = self.sidebar.view(self.scale, now);
        app.set_sidebar_collapsed(sv.collapsed);
        app.set_sidebar_revealed(sv.revealed);
        app.set_sidebar_width(sv.width_css as f32);
        app.set_sidebar_sliding(sv.sliding);
        if sv.frozen {
            if app.get_grid_frozen_width() <= 0.0 {
                let w = app.get_grid_phys_width() as f32 / self.scale as f32;
                app.set_grid_frozen_width(w);
            }
        } else if app.get_grid_frozen_width() > 0.0 {
            app.set_grid_frozen_width(0.0);
        }

        self.view = view;
        if self.overflow.is_some() {
            self.push_overflow();
        }

        // The grid draws whichever shell is in front; Settings, a placeholder and no tab draw none.
        let want = self
            .model
            .active_shell()
            .and_then(|t| self.deps.host.handle(&t.id).map(|h| (t.id.clone(), h)));
        let want_id = want.as_ref().map(|w| w.0.clone());
        if want_id != self.source_id {
            self.source_id = want_id;
            (self.deps.set_grid_source)(want.map(|w| w.1));
        }

        // Rule 37: nothing is written before the restore answered.
        if let Some(s) = self.model.saved_if_restored()
            && self.last_saved.as_ref() != Some(&s)
        {
            self.deps.tabs_store.save(&s);
            self.last_saved = Some(s);
        }

        app.window().request_redraw();
        self.kick();
    }

    // ---- the restore ----------------------------------------------------------------------------

    /// A worker thread resolves the saved strip; [`UiMsg::RestoreDone`] answers. Runs ONCE.
    pub fn start_restore(this: &Rc<RefCell<AppCtl>>) {
        let (saved, is_dir, lookup, post) = {
            let mut c = this.borrow_mut();
            if c.restore_started {
                return;
            }
            c.restore_started = true;
            (
                c.boot_saved.clone(),
                Arc::clone(&c.deps.is_dir),
                Arc::clone(&c.deps.lookup),
                Arc::clone(&c.deps.post),
            )
        };
        let r = std::thread::Builder::new()
            .name("flare-restore".into())
            .spawn({
                let post = Arc::clone(&post);
                move || {
                    let plan = resolve_restore(&saved, is_dir, lookup);
                    post(UiMsg::RestoreDone(plan));
                }
            });
        if let Err(e) = r {
            log::warn!("could not start the restore worker: {e}");
            post(UiMsg::RestoreDone(Restored::default()));
        }
    }

    fn restored(&mut self, mut plan: Restored) {
        with_launch_folders(&mut plan, &self.deps.launch_folders);
        let spawns = self.model.settle(&plan);
        for s in spawns {
            self.deps.host.spawn(&s.id, &s.cwd, s.resume.as_deref());
            match s.resume.as_deref() {
                Some(CODEX_RESUME) => self.model.set_agent(&s.id, Some(AgentKind::Codex)),
                Some(_) => self.model.set_agent(&s.id, Some(AgentKind::Claude)),
                None => {}
            }
        }
        let queued = self.pending.restored();
        for folder in queued {
            self.open_folder(&folder);
        }
        self.prewarm_new_tab();
        self.refresh();
    }

    // ---- opening and closing tabs -----------------------------------------------------------------

    /// A new tab in `dir`, shell started. Never blocks.
    fn open_folder(&mut self, dir: &str) -> String {
        let id = self.model.open_tab(dir);
        self.deps.host.spawn(&id, dir, None);
        self.refresh();
        id
    }

    fn open_recent(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.close_menu_quietly();
        self.open_folder(path);
        self.deps.host.prewarm(path);
        self.focus_terminal();
    }

    fn new_tab_dir(&self) -> String {
        let stored = new_tab_folder(&self.prefs);
        if stored.is_empty() {
            self.deps.home.clone()
        } else {
            stored
        }
    }

    fn prewarm_new_tab(&self) {
        if new_tab_mode(&self.prefs) == NewTabMode::Folder {
            self.deps.host.prewarm(&self.new_tab_dir());
        }
    }

    /// The + button and Ctrl+T. Folder mode: the stored folder, else the user's folder, no
    /// question. Ask mode: the folder picker (a cancel opens nothing).
    pub fn new_tab(&mut self) {
        self.close_menu_quietly();
        match new_tab_mode(&self.prefs) {
            NewTabMode::Folder => {
                let dir = self.new_tab_dir();
                self.open_folder(&dir);
                self.deps.host.prewarm(&dir);
                self.focus_terminal();
            }
            NewTabMode::Ask => {
                let stored = new_tab_folder(&self.prefs);
                let default_dir = (!stored.is_empty()).then(|| PathBuf::from(stored));
                let post = Arc::clone(&self.deps.post);
                let answer: PickAnswer = Arc::new(move |p| post(UiMsg::Picked(p)));
                (self.deps.pick_folder)(default_dir, answer);
            }
        }
    }

    /// Closing asks first when the tab hosts an agent, else closes at once.
    pub fn request_close(&mut self, id: &str) {
        if !self.model.state.tabs.iter().any(|t| t.id == id) {
            return;
        }
        if flare_agent::close::asks_before_closing_tab(self.model.has_agent(id)) {
            let label = self
                .model
                .state
                .tabs
                .iter()
                .position(|t| t.id == id)
                .and_then(|i| self.model.labels().get(i).cloned())
                .unwrap_or_default();
            let for_ms = self
                .working
                .get(id)
                .map(|since| self.now().saturating_sub(*since));
            self.ask = Some(tab_ask(
                id,
                &label,
                detected(self.model.agent_kind(id)),
                for_ms,
            ));
            self.show_ask();
            return;
        }
        self.close_now(id);
    }

    fn close_now(&mut self, id: &str) {
        self.deps.host.kill(id);
        self.working.remove(id);
        self.model.close(id);
        self.refresh();
    }

    fn toggle_settings(&mut self) {
        self.close_menu_quietly();
        self.model.toggle_settings();
        self.refresh();
        self.focus_terminal();
    }

    // ---- strip and sidebar drags -------------------------------------------------------------------

    fn strip_press(&mut self, id: &str, row_index: i32, x: f64, origin: f64) {
        self.clear_tip();
        let strip_phys = self.app().map_or(0, |a| a.get_strip_phys_width());
        let took = self.strip_drag.press(
            id,
            usize::try_from(row_index).unwrap_or(usize::MAX),
            x,
            origin,
            &self.view,
            strip_phys,
            self.scale,
        );
        self.pressed = took.then(|| id.to_string());
    }

    fn strip_moved(&mut self, x: f64) {
        self.strip_drag.moved(x);
        self.refresh();
    }

    fn strip_released(&mut self) {
        let release = self.strip_drag.release();
        let pressed = self.pressed.take();
        match release {
            Release::Pick => {
                if let Some(id) = pressed {
                    self.model.pick(&id);
                }
            }
            Release::Drop { id, slot } => {
                if let Some(to) = slot_to_model(&self.model, &self.view, slot) {
                    self.model.reorder(&id, to);
                }
            }
            Release::Nothing => {}
        }
        self.refresh();
        self.focus_terminal();
    }

    fn side_press(&mut self, id: &str, row_index: i32, p: [f64; 4]) {
        self.clear_tip();
        let [y, first_row_top, rows_top, rows_bottom] = p;
        let refuse = !self.query.trim().is_empty();
        let took = self.side_drag.press(
            id,
            usize::try_from(row_index).unwrap_or(usize::MAX),
            y,
            first_row_top,
            rows_top,
            rows_bottom,
            self.view.rows.len(),
            refuse,
        );
        self.pressed = took.then(|| id.to_string());
    }

    fn side_moved(&mut self, y: f64) {
        self.side_drag.moved(y);
        self.refresh();
    }

    fn side_released(&mut self) {
        let release = self.side_drag.release();
        let pressed = self.pressed.take();
        match release {
            Release::Pick => {
                if let Some(id) = pressed {
                    self.model.pick(&id);
                }
            }
            Release::Drop { id, slot } => self.model.reorder(&id, slot),
            Release::Nothing => {}
        }
        self.refresh();
        self.focus_terminal();
    }

    // ---- the sidebar -------------------------------------------------------------------------------

    pub fn toggle_sidebar(&mut self) {
        let now = self.now();
        self.sidebar.toggle(&self.prefs, now);
        self.refresh();
    }

    fn grip_moved(&mut self, w: f64) {
        let width = self.sidebar.grip_moved(w);
        if let Some(app) = self.app() {
            app.set_sidebar_width(width as f32);
        }
        self.kick();
    }

    fn grip_released(&mut self, w: f64) {
        self.sidebar.grip_released(&self.prefs, w);
        self.refresh();
    }

    fn find_edited(&mut self, q: &str) {
        self.query = q.to_string();
        self.refresh();
    }

    /// Enter in the find box fronts the first tab that matches.
    fn find_accepted(&mut self) {
        let first = self.view.rows.iter().find(|r| r.kept).map(|r| r.id.clone());
        if let Some(id) = first {
            self.model.pick(&id);
            // PT TabSidebar.tsx clears the query on Enter, like Escape does.
            self.query.clear();
            if let Some(app) = self.app() {
                app.set_sidebar_find("".into());
            }
            self.refresh();
            self.focus_terminal();
        }
    }

    /// Escape in the find box clears it and gives the keys back to the terminal.
    fn find_escape(&mut self) {
        self.query.clear();
        if let Some(app) = self.app() {
            app.set_sidebar_find("".into());
        }
        self.refresh();
        self.focus_terminal();
    }

    fn toggle_maximise(&mut self) {
        if let Some(app) = self.app() {
            let w = app.window();
            w.set_maximized(!w.is_maximized());
        }
    }

    fn title_pressed(&mut self) {
        self.clear_tip();
        self.close_menu_quietly();
        self.close_overflow_quietly();
        self.refresh();
    }

    // ---- menus, the +N list and the start screen -------------------------------------------------------

    fn open_plus_menu(&mut self, x: f32, y: f32) {
        self.close_overflow_quietly();
        let rows = plus_menu_rows(&self.prefs);
        self.show_menu(MenuKind::Plus, rows, x, y);
    }

    fn open_tab_menu(&mut self, id: &str, x: f32, y: f32) {
        self.close_overflow_quietly();
        let Some(tab) = self.model.state.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        let cwd = if tab.kind == TabKind::Settings {
            String::new()
        } else {
            tab.cwd.clone()
        };
        let rows = tab_menu_rows(&cwd, self.key_label_of(ChordId::CloseTab))
            .into_iter()
            .map(|r| (r, String::new()))
            .collect();
        self.show_menu(
            MenuKind::Tab {
                id: id.to_string(),
                cwd,
            },
            rows,
            x,
            y,
        );
    }

    fn show_menu(&mut self, kind: MenuKind, rows: Vec<(MenuRowData, String)>, x: f32, y: f32) {
        self.clear_tip();
        self.menu = Some(MenuState { kind, rows });
        if let Some(app) = self.app() {
            app.set_menu_x(x);
            app.set_menu_y(y);
        }
        self.push_menu();
        self.refresh();
    }

    fn push_menu(&self) {
        let Some(app) = self.app() else { return };
        match &self.menu {
            Some(m) => {
                let rows: Vec<MenuRow> = m.rows.iter().map(|(r, _)| menu_row(r)).collect();
                app.set_menu_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
                app.set_menu_open(true);
            }
            None => app.set_menu_open(false),
        }
    }

    fn close_menu_quietly(&mut self) {
        if self.menu.take().is_some() {
            self.push_menu();
        }
    }

    fn close_menu(&mut self) {
        self.close_menu_quietly();
        self.refresh();
    }

    fn menu_picked(&mut self, i: i32) {
        let Some(m) = self.menu.take() else { return };
        self.push_menu();
        let Some((row, path)) = usize::try_from(i).ok().and_then(|i| m.rows.get(i)) else {
            self.refresh();
            return;
        };
        match &m.kind {
            MenuKind::Plus => {
                if !path.is_empty() {
                    self.open_recent(path);
                }
            }
            MenuKind::Tab { id, cwd } => match row.label.as_str() {
                "Close tab" => self.request_close(id),
                "Copy folder path" => (self.deps.clipboard)(cwd),
                _ => {}
            },
        }
        self.refresh();
    }

    /// The pin at the end of a + menu row: pin or unpin, the menu stays.
    fn menu_trailing(&mut self, i: i32) {
        let Some(m) = self.menu.as_mut() else { return };
        if !matches!(m.kind, MenuKind::Plus) {
            return;
        }
        if let Ok(i) = usize::try_from(i) {
            toggle_pin_in_menu(&self.prefs, &mut m.rows, i);
        }
        self.push_menu();
    }

    fn open_overflow(&mut self, all: bool, right: f32, bottom: f32) {
        self.close_menu_quietly();
        self.clear_tip();
        self.overflow = Some(OverflowState {
            all,
            query: String::new(),
        });
        if let Some(app) = self.app() {
            app.set_overflow_right(right);
            app.set_overflow_bottom(bottom);
            app.set_overflow_query("".into());
        }
        self.push_overflow();
        self.refresh();
    }

    fn push_overflow(&self) {
        let Some(app) = self.app() else { return };
        match &self.overflow {
            Some(o) => {
                let rows: Vec<OverflowRow> = overflow_rows(&self.view, o.all, &o.query)
                    .iter()
                    .map(overflow_row)
                    .collect();
                app.set_overflow_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
                app.set_overflow_open(true);
            }
            None => app.set_overflow_open(false),
        }
    }

    fn close_overflow_quietly(&mut self) {
        if self.overflow.take().is_some() {
            self.push_overflow();
        }
    }

    fn close_overflow(&mut self) {
        self.close_overflow_quietly();
        self.refresh();
        self.focus_terminal();
    }

    fn overflow_query(&mut self, q: &str) {
        if let Some(o) = self.overflow.as_mut() {
            o.query = q.to_string();
        }
        self.push_overflow();
    }

    fn overflow_picked(&mut self, id: &str) {
        self.close_overflow_quietly();
        self.model.pick(id);
        self.refresh();
        self.focus_terminal();
    }

    /// Enter in the +N list fronts the first row.
    fn overflow_accepted(&mut self) {
        let first = self.overflow.as_ref().and_then(|o| {
            overflow_rows(&self.view, o.all, &o.query)
                .into_iter()
                .next()
                .map(|r| r.id)
        });
        match first {
            Some(id) => self.overflow_picked(&id),
            None => self.close_overflow(),
        }
    }

    fn overflow_row_menu(&mut self, id: &str, x: f32, y: f32) {
        self.close_overflow_quietly();
        self.open_tab_menu(id, x, y);
    }

    fn open_github(&mut self) {
        let url = "https://github.com/Maxaubert/Flare";
        if flare_term::web_url::is_web_url(url) {
            (self.deps.open_url)(url);
        }
    }

    // ---- tooltips ---------------------------------------------------------------------------------------

    fn tip_request(&mut self, text: &str, x: f32, y: f32, w: f32, h: f32) {
        if text.is_empty() {
            self.clear_tip();
            return;
        }
        let now = self.now();
        let side_width = self.sidebar.view(self.scale, now).width_css as f32;
        let prefer = if self.vertical() && x < side_width {
            TipSide::Right
        } else {
            TipSide::Below
        };
        let item = BoxF {
            left: f64::from(x),
            top: f64::from(y),
            width: f64::from(w),
            height: f64::from(h),
        };
        self.tips.request(text, item, prefer, now);
        self.kick();
    }

    fn clear_tip(&mut self) {
        self.tips.clear();
        if let Some(app) = self.app() {
            app.set_tip_visible(false);
        }
    }

    fn tip_tick(&mut self, now: u64) {
        let Some(due) = self.tips.next_due() else {
            return;
        };
        if now < due {
            return;
        }
        let Some(app) = self.app() else { return };
        let Some(text) = self.tips.text().map(str::to_string) else {
            return;
        };
        // The tip is measured with its text in place, then placed.
        app.set_tip_text(text.as_str().into());
        let size = (f64::from(app.get_tip_w()), f64::from(app.get_tip_h()));
        let win = app.window().size();
        let view = (
            f64::from(win.width) / self.scale,
            f64::from(win.height) / self.scale,
        );
        if let Some(spot) = self.tips.poll(now, size, view) {
            app.set_tip_x(spot.x as f32);
            app.set_tip_y(spot.y as f32);
            app.set_tip_side(match spot.side {
                TipSide::Below => 0,
                TipSide::Above => 1,
                TipSide::Right => 2,
                TipSide::Left => 3,
            });
            app.set_tip_visible(true);
        }
    }

    // ---- the timer --------------------------------------------------------------------------------------------

    fn next_due(&self) -> Option<u64> {
        [self.sidebar.next_due(), self.tips.next_due()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Starts the 50 ms timer when something is pending; an idle Flare wakes up for nothing.
    fn kick(&mut self) {
        if self.timer_on || self.next_due().is_none() {
            return;
        }
        let weak = self.me.clone();
        self.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(TICK_MS),
            move || {
                if let Some(rc) = weak.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.tick();
                }
            },
        );
        self.timer_on = true;
    }

    /// The timer's body; stops the timer when nothing is pending.
    pub fn tick(&mut self) {
        let now = self.now();
        let events = self.sidebar.tick(now);
        let changed = !events.is_empty();
        for ev in events {
            if ev == SidebarEvent::FitDue
                && let Some(app) = self.app()
            {
                // One fit at the end of the slide (D9): the grid takes its true width back.
                app.set_grid_frozen_width(0.0);
            }
        }
        self.tip_tick(now);
        if changed {
            self.refresh();
        }
        if self.next_due().is_none() {
            self.timer.stop();
            self.timer_on = false;
        }
    }

    // ---- keys -----------------------------------------------------------------------------------------------------

    /// The chord context (PT App.tsx onKey): command help is M7's, the agent fields are M6's.
    pub fn key_context(&self) -> KeyContext {
        KeyContext {
            help: false,
            vertical: self.vertical(),
            full_screen: self.front_handle().is_some_and(|h| h.mode().alt_screen),
            agent_keys: agent_keys(&self.prefs),
            agent_guard: false,
            agent_here: false,
            agent_working: false,
            agent_selecting: false,
        }
    }

    /// Order: the chord table first; then a Slint control with the keys, else the front shell.
    pub fn on_key(&mut self, ev: &KeyEvent) -> KeyOutcome {
        let ctx = self.key_context();
        let d = chords::dispatch(ev, &ctx, &mut Target(self));
        let shell_in_front = self.front_handle().is_some();
        match d {
            Dispatch::Taken => KeyOutcome::Consumed,
            Dispatch::ActedShellToo => {
                if shell_in_front {
                    KeyOutcome::Pty
                } else {
                    KeyOutcome::Consumed
                }
            }
            Dispatch::NotAChord => {
                if self.app().is_some_and(|a| a.get_chrome_has_focus()) {
                    KeyOutcome::Slint
                } else if shell_in_front {
                    KeyOutcome::Pty
                } else {
                    KeyOutcome::Consumed
                }
            }
        }
    }

    pub fn on_key_up(&mut self, ev: &KeyEvent) {
        chords::on_key_up(ev, &mut Target(self));
    }

    /// The window lost the keyboard: a Most recent walk ends, tips go.
    pub fn on_focus_lost(&mut self) {
        self.model.commit_walk();
        self.clear_tip();
    }

    // ---- pointer and files ------------------------------------------------------------------------------------------

    /// The pointer in window css pixels.
    pub fn on_cursor(&mut self, x_css: f32, y_css: f32) {
        if self.tips.shown() {
            self.clear_tip();
        }
        let now = self.now();
        self.sidebar.pointer(
            Some(Point {
                x: f64::from(x_css),
                y: f64::from(y_css),
            }),
            now,
        );
        if self.flight.on() {
            self.flight.pointer_moved();
            self.refresh();
        }
        self.kick();
    }

    /// The pointer left the window (a collapsed sidebar lets go).
    pub fn on_cursor_left(&mut self) {
        let now = self.now();
        self.sidebar.pointer(None, now);
        self.kick();
    }

    pub fn on_pointer_down(&mut self) {
        self.clear_tip();
        if self.flight.on() {
            self.flight.pointer_down();
            self.refresh();
        }
    }

    pub fn on_hovered_file(&mut self) {
        self.flight.hovered_file();
        self.refresh();
    }

    pub fn on_hovered_file_cancelled(&mut self) {
        self.flight.cancelled();
        self.refresh();
    }

    /// Over the tab row or the sidebar a worker stats the path (a file means the folder that holds
    /// it) and a tab opens there; elsewhere the drop is M5's.
    pub fn on_dropped_path(&mut self, path: PathBuf, cursor_css: (f32, f32)) {
        self.flight.dropped();
        self.refresh();
        let side_width = self.sidebar.view(self.scale, self.now()).width_css as f32;
        let on_strip = !self.vertical() && cursor_css.1 < BAR_H;
        let on_side = self.vertical()
            && self.view.has_tabs
            && sidebar_mode(&self.prefs) == SidebarMode::Open
            && cursor_css.0 < side_width;
        if !(on_strip || on_side) {
            return;
        }
        let is_dir = Arc::clone(&self.deps.is_dir);
        let post = Arc::clone(&self.deps.post);
        let r = std::thread::Builder::new()
            .name("flare-drop".into())
            .spawn(move || {
                let as_text = |p: &Path| p.to_string_lossy().into_owned();
                let dir = if is_dir(&as_text(&path)) {
                    Some(as_text(&path))
                } else {
                    path.parent().map(as_text).filter(|p| is_dir(p))
                };
                if let Some(dir) = dir {
                    post(UiMsg::FolderResolved(dir));
                }
            });
        if let Err(e) = r {
            log::warn!("could not start the drop worker: {e}");
        }
    }

    // ---- the window ---------------------------------------------------------------------------------------------------

    /// The window moved, resized or changed mode: remember the normal bounds (saved on a delay).
    pub fn on_geometry(&mut self, o: Observed) {
        self.bounds.observe(&o);
        self.maximised = o.maximised;
        if let Some(state) = self.bounds.state(o.maximised) {
            self.deps.window_store.save(&state);
        }
    }

    /// The grid area changed (Task 30b's `grid-resized`, after the grid host resized): tabs born
    /// from now on start at this size and the front tab follows.
    pub fn on_grid_dims(&mut self, cols: usize, rows: usize) {
        self.grid_dims = (cols, rows);
        let front = self.front_id();
        self.deps.host.set_dims(cols, rows, front.as_deref());
    }

    /// Folders from a second launch: raise the window, then each folder is a new tab.
    pub fn handoff(&mut self, h: Handoff) {
        (self.deps.raise)();
        let post = Arc::clone(&self.deps.post);
        let r = std::thread::Builder::new()
            .name("flare-handoff".into())
            .spawn(move || {
                let folders = folders_from_argv(&h.args, &own_files(), Path::new(&h.cwd));
                post(UiMsg::Folders(folders));
            });
        if let Err(e) = r {
            log::warn!("could not start the hand-off worker: {e}");
        }
    }

    fn save_window_now(&mut self) {
        if let Some(state) = self.bounds.state(self.maximised) {
            self.deps.window_store.save(&state);
        }
    }

    /// Every route out of the window ends here. A working agent vetoes the close and asks.
    pub fn on_close_requested(&mut self) -> CloseDecision {
        let now = self.now();
        let busy: Vec<(String, DetectedAgent, u64)> = self
            .working
            .iter()
            .filter(|(id, _)| self.model.has_agent(id))
            .map(|(id, since)| {
                let label = self
                    .model
                    .state
                    .tabs
                    .iter()
                    .position(|t| &t.id == id)
                    .and_then(|i| self.model.labels().get(i).cloned())
                    .unwrap_or_default();
                (
                    label,
                    detected(self.model.agent_kind(id)),
                    now.saturating_sub(*since),
                )
            })
            .collect();
        self.close_flow.set_busy(holds_window_close(busy.len()));
        match self.close_flow.on_close() {
            CloseVerdict::Ask => {
                self.ask = window_ask(&busy);
                (self.deps.raise)();
                self.show_ask();
                CloseDecision::KeepOpen
            }
            CloseVerdict::Close => {
                self.finish();
                CloseDecision::Close
            }
        }
    }

    /// After the event loop: the end of life without asking (idempotent).
    pub fn end(&mut self) {
        self.finish();
    }

    /// The end of life: both state files flushed, then every shell killed and waited for.
    fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.save_window_now();
        if let Some(s) = self.model.saved_if_restored() {
            self.deps.tabs_store.save(&s);
        }
        self.prefs.flush();
        (self.deps.hide)();
        end_of_life(
            &self.deps.tabs_store,
            &self.deps.window_store,
            &mut self.deps.host,
        );
    }

    // ---- the close question ------------------------------------------------------------------------------------------------

    fn show_ask(&mut self) {
        self.clear_tip();
        self.close_menu_quietly();
        self.close_overflow_quietly();
        if let Some(app) = self.app() {
            match &self.ask {
                Some(a) => {
                    let v = ask_view(a);
                    let body = slint::StyledText::from_markdown(&v.body)
                        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&v.body));
                    app.set_dialog_title(v.title.into());
                    app.set_dialog_body(body);
                    app.set_dialog_primary(v.primary.into());
                    app.set_dialog_danger(true);
                    app.set_dialog_open(true);
                }
                None => app.set_dialog_open(false),
            }
        }
        self.refresh();
    }

    fn dialog_cancel(&mut self) {
        self.ask = None;
        self.show_ask();
        self.focus_terminal();
    }

    fn dialog_confirm(&mut self) {
        let Some(ask) = self.ask.take() else { return };
        self.show_ask();
        match ask.target {
            crate::startup::AskTarget::Tab(id) => {
                self.close_now(&id);
                self.focus_terminal();
            }
            crate::startup::AskTarget::Window => {
                match self.close_flow.confirm() {
                    Confirmed::Quit | Confirmed::Close => {}
                }
                self.finish();
                (self.deps.quit)();
            }
        }
    }

    // ---- worker answers -----------------------------------------------------------------------------------------------------

    /// The one entry for worker answers.
    pub fn on_msg(&mut self, m: UiMsg) {
        // After the end of life nothing may raise the window or start a shell: a hand-off, a
        // picked folder or a late restore answer would spawn a shell the quit never waits for.
        if self.ended {
            return;
        }
        match m {
            UiMsg::RestoreDone(plan) => self.restored(plan),
            UiMsg::Picked(Some(dir)) => {
                let dir = dir.to_string_lossy().into_owned();
                self.open_folder(&dir);
                self.focus_terminal();
            }
            UiMsg::Picked(None) => {}
            UiMsg::FolderResolved(dir) => {
                self.open_folder(&dir);
                self.focus_terminal();
            }
            UiMsg::Folders(folders) => {
                for f in self.pending.push(folders) {
                    self.open_folder(&f);
                }
                self.focus_terminal();
            }
            UiMsg::Handoff(h) => self.handoff(h),
            UiMsg::HostWake => self.host_wake(),
        }
    }

    /// The waker: drain what the shells reported. A folder report that is a share or holds a
    /// control character is never adopted (a program must not steer a tab to SMB), and a shell
    /// that exited closes its tab unasked.
    pub fn host_wake(&mut self) {
        let events = self.deps.host.drain_events();
        let mut changed = false;
        for ev in events {
            match ev {
                HostEvent::Cwd { id, path } => {
                    if !is_unsafe_restore_folder(&path) && self.model.set_cwd(&id, &path) {
                        changed = true;
                    }
                }
                HostEvent::Exited { id, .. } => {
                    self.deps.host.kill(&id);
                    self.working.remove(&id);
                    if self.model.close(&id) {
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed {
            self.refresh();
        } else if let Some(app) = self.app() {
            app.window().request_redraw();
        }
    }
}

/// The chord actions, acting on the controller.
struct Target<'a>(&'a mut AppCtl);

impl ChordTarget for Target<'_> {
    fn blocked(&self) -> bool {
        self.0.ask.is_some()
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(app) = self.0.app() {
            let w = app.window();
            w.set_fullscreen(!w.is_fullscreen());
        }
    }

    fn toggle_help(&mut self) {
        // Command help is M7's.
    }

    fn commit_walk(&mut self) {
        self.0.model.commit_walk();
    }

    fn next_tab(&mut self, dir: i32) {
        let switch = tab_switch(&self.0.prefs);
        self.0.model.step(dir, switch);
        self.0.refresh();
    }

    fn jump_tab(&mut self, n: usize) -> bool {
        let ok = self.0.model.jump(n);
        if ok {
            self.0.refresh();
        }
        ok
    }

    fn toggle_settings(&mut self) {
        self.0.toggle_settings();
    }

    fn find_tab(&mut self) {
        if self.0.vertical() {
            if let Some(app) = self.0.app() {
                app.invoke_focus_sidebar_find();
            }
            self.0.refresh();
        } else {
            let width = self.0.app().map_or(0.0, |a| {
                a.window().size().width as f32 / self.0.scale as f32
            });
            self.0
                .open_overflow(true, width - CHORD_LIST_RIGHT_GAP, BAR_H);
        }
    }

    fn toggle_sidebar(&mut self) {
        self.0.toggle_sidebar();
    }

    fn new_tab(&mut self) {
        self.0.new_tab();
    }

    fn close_tab(&mut self) {
        if let Some(id) = self.0.front_id() {
            self.0.request_close(&id);
        }
    }

    fn find(&mut self) {
        // Find in the terminal is M5's.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_harness::Harness;
    use flare_settings::new_tab::set_new_tab_mode;
    use flare_settings::tab_layout::set_tab_layout;
    use flare_settings::term_prefs::set_agent_keys;
    use flare_tabs::restore::{NoSessions, SavedTab, tabs_json};
    use flare_term::fake::{FakeShells, FakeSpawner};
    use flare_win::window_state::{Area, parse_window_state};
    use slint::Model;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

    #[derive(Default)]
    struct Log {
        clipboard: Vec<String>,
        urls: Vec<String>,
        raised: usize,
        quit: usize,
        hidden: usize,
        sources: Vec<Option<String>>,
        picks: Vec<Option<PathBuf>>,
    }

    #[derive(Default)]
    struct Opts {
        saved: Vec<(&'static str, Option<AgentKind>)>,
        active: usize,
        launch: Vec<String>,
        real_fs: bool,
        pick_answer: Option<PathBuf>,
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
    }

    fn fx_with(opts: Opts) -> Fx {
        fx_with_prefs(opts, |_| {})
    }

    fn fx_with_prefs(opts: Opts, prep: impl FnOnce(&PrefStore)) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        if !opts.saved.is_empty() {
            let saved = SavedTabs {
                tabs: opts
                    .saved
                    .iter()
                    .map(|(cwd, agent)| SavedTab {
                        cwd: (*cwd).to_string(),
                        agent: *agent,
                    })
                    .collect(),
                active: opts.active,
            };
            std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test
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
        let host = TabHost::new(sp.clone(), FakeShells::new(), prefs.clone(), waker);
        let clock = Rc::new(Cell::new(0u64));
        let log: Rc<RefCell<Log>> = Rc::default();
        let real_fs = opts.real_fs;
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = if real_fs {
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
            home: "C:\\home".to_string(),
            clock: {
                let c = clock.clone();
                Rc::new(move || c.get())
            },
            post,
            pick_folder: {
                let log = log.clone();
                Rc::new(move |_default, done| {
                    log.borrow_mut().picks.push(_default);
                    done(answer.clone());
                })
            },
            clipboard: {
                let log = log.clone();
                Rc::new(move |s| log.borrow_mut().clipboard.push(s.to_string()))
            },
            open_url: {
                let log = log.clone();
                Rc::new(move |s| log.borrow_mut().urls.push(s.to_string()))
            },
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
            launch_folders: opts.launch,
            e2e: true,
        };
        let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
        let ctl = AppCtl::new(&h.comp, deps);
        Fx {
            h,
            ctl,
            q,
            sp,
            dir,
            clock,
            log,
            prefs,
        }
    }

    fn fx() -> Fx {
        fx_with(Opts::default())
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

        fn start_restore_and_wait(&self) {
            AppCtl::start_restore(&self.ctl);
            self.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
        }

        fn rows(&self) -> usize {
            self.h.comp.get_tab_rows().row_count()
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

        fn key(&self, ev: KeyEvent) -> KeyOutcome {
            self.ctl.borrow_mut().on_key(&ev)
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
    }

    fn ctrl(key: &str, code: &str) -> KeyEvent {
        KeyEvent::new(key, code).ctrl()
    }

    #[test]
    fn boot_draws_the_saved_strip_before_the_restore_answers() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", Some(AgentKind::Claude))],
            ..Opts::default()
        });
        assert_eq!(f.rows(), 2);
        let rows = f.h.comp.get_tab_rows();
        assert!(!rows.row_data(0).unwrap().loading);
        assert!(rows.row_data(1).unwrap().loading);
        assert_eq!(f.h.comp.get_front_kind(), 3);
        assert_eq!(f.sp.made().len(), 0);
    }

    #[test]
    fn the_restore_spawns_every_shell_and_settles_in_place() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", None)],
            active: 1,
            ..Opts::default()
        });
        f.start_restore_and_wait();
        f.pump_until("both spawns", |f| f.sp.made().len() >= 2);
        let mut spawned: Vec<String> = f.sp.made().iter().map(|m| m.spec().cwd).collect();
        spawned.sort();
        assert!(spawned.starts_with(&["C:\\a".to_string(), "C:\\b".to_string()]));
        assert_eq!(f.cwds(), ["C:\\a", "C:\\b"]);
        assert_eq!(f.h.comp.get_front_kind(), 0);
        let ctl = f.ctl.borrow();
        assert!(ctl.model().state.tabs.iter().all(|t| t.pending.is_none()));
        assert_eq!(ctl.model().active().unwrap().cwd, "C:\\b");
        assert!(ctl.model().restore_done());
    }

    #[test]
    fn a_gone_folder_is_dropped_silently() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\gone", None)],
            active: 1,
            ..Opts::default()
        });
        f.start_restore_and_wait();
        assert_eq!(f.rows(), 1);
        assert_eq!(f.cwds(), ["C:\\a"]);
        assert_eq!(f.ctl.borrow().model().active().unwrap().cwd, "C:\\a");
    }

    #[test]
    fn launch_folders_join_after_the_restored_tabs() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            launch: vec!["D:\\x".to_string()],
            ..Opts::default()
        });
        f.start_restore_and_wait();
        assert_eq!(f.cwds(), ["C:\\a", "D:\\x"]);
        assert_eq!(f.ctl.borrow().model().active().unwrap().cwd, "D:\\x");
    }

    fn file_tabs(f: &Fx) -> Option<usize> {
        let raw = std::fs::read_to_string(f.dir.path().join("tabs.json")).ok()?; // sync-ok: test
        Some(flare_tabs::restore::parse_tabs(&raw).tabs.len())
    }

    #[test]
    fn nothing_is_saved_before_the_restore_and_the_last_report_wins_after() {
        let f = fx();
        f.ctl.borrow_mut().new_tab();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(file_tabs(&f), None, "nothing before the restore answered");
        f.start_restore_and_wait();
        for _ in 0..3 {
            f.ctl.borrow_mut().new_tab();
        }
        let n = f.rows();
        f.pump_until("the file", |f| file_tabs(f) == Some(n));
        assert_eq!(n, 4);
    }

    #[test]
    fn ctrl_t_opens_a_tab_in_the_users_folder_without_asking() {
        let f = fx();
        assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
        f.wait_made("C:\\home");
        assert_eq!(f.cwds(), ["C:\\home"]);
        assert!(f.log.borrow().picks.is_empty());
    }

    #[test]
    fn ask_mode_uses_the_picker_and_cancel_opens_nothing() {
        let f = fx_with_prefs(
            Opts {
                pick_answer: Some(PathBuf::from("D:\\w")),
                ..Opts::default()
            },
            |p| set_new_tab_mode(p, NewTabMode::Ask, Some("D:\\stored")),
        );
        f.ctl.borrow_mut().new_tab();
        assert_eq!(
            f.log.borrow().picks,
            vec![Some(PathBuf::from("D:\\stored"))]
        );
        f.pump_until("the tab", |f| f.rows() == 1);
        assert_eq!(f.cwds(), ["D:\\w"]);

        let g = fx_with_prefs(Opts::default(), |p| {
            set_new_tab_mode(p, NewTabMode::Ask, None);
        });
        g.ctl.borrow_mut().new_tab();
        g.pump();
        assert_eq!(g.log.borrow().picks, vec![None]);
        assert_eq!(g.rows(), 0);
    }

    #[test]
    fn a_dropped_folder_on_the_tab_row_opens_a_tab_and_a_file_opens_its_folder() {
        let f = fx_with(Opts {
            real_fs: true,
            ..Opts::default()
        });
        let folder = f.dir.path().join("proj");
        std::fs::create_dir(&folder).unwrap(); // sync-ok: test
        let file = folder.join("a.txt");
        std::fs::write(&file, "x").unwrap(); // sync-ok: test
        f.ctl
            .borrow_mut()
            .on_dropped_path(folder.clone(), (100.0, 10.0));
        f.pump_until("the first tab", |f| f.rows() == 1);
        f.ctl.borrow_mut().on_dropped_path(file, (100.0, 10.0));
        f.pump_until("the second tab", |f| f.rows() == 2);
        let want = folder.to_string_lossy().into_owned();
        assert_eq!(f.cwds(), [want.clone(), want]);
        // Over the terminal area the drop is M5's.
        f.ctl.borrow_mut().on_dropped_path(folder, (500.0, 300.0));
        std::thread::sleep(Duration::from_millis(100));
        f.pump();
        assert_eq!(f.rows(), 2);
    }

    #[test]
    fn exit_closes_the_tab_without_asking() {
        let f = fx();
        f.ctl.borrow_mut().open_folder("C:\\a");
        f.ctl.borrow_mut().open_folder("C:\\b");
        f.wait_made("C:\\a");
        f.wait_made("C:\\b");
        f.made_with("C:\\b")[0].exit(0);
        f.pump_until("the tab to close", |f| f.rows() == 1);
        assert_eq!(f.cwds(), ["C:\\a"]);
        assert_eq!(f.ctl.borrow().model().active().unwrap().cwd, "C:\\a");
    }

    #[test]
    fn closing_the_last_tab_shows_the_start_screen_and_keeps_the_window() {
        let f = fx();
        f.start_restore_and_wait();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        f.ctl.borrow_mut().request_close(&id);
        assert_eq!(f.rows(), 0);
        assert_eq!(f.h.comp.get_front_kind(), 2);
        assert_eq!(f.log.borrow().quit, 0);
        assert!(f.h.comp.get_places().row_count() >= 1);
    }

    #[test]
    fn a_tab_hosting_an_agent_asks_before_closing() {
        let f = fx();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        f.wait_made("C:\\a");
        f.ctl
            .borrow_mut()
            .model_mut()
            .set_agent(&id, Some(AgentKind::Claude));
        assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
        assert!(f.h.comp.get_dialog_open());
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Close the tab and end the agent?"
        );
        f.h.comp.invoke_dialog_cancel();
        assert!(!f.h.comp.get_dialog_open());
        assert_eq!(f.rows(), 1);
        f.key(ctrl("w", "KeyW"));
        f.h.comp.invoke_dialog_confirm();
        assert_eq!(f.rows(), 0);
        assert!(!f.h.comp.get_dialog_open());
        f.pump_until("the shell to be killed", |f| {
            f.made_with("C:\\a")[0].killed()
        });
    }

    #[test]
    fn the_window_close_is_held_only_while_an_agent_works() {
        let idle = fx();
        let id = idle.ctl.borrow_mut().open_folder("C:\\a");
        idle.ctl
            .borrow_mut()
            .model_mut()
            .set_agent(&id, Some(AgentKind::Claude));
        assert_eq!(
            idle.ctl.borrow_mut().on_close_requested(),
            CloseDecision::Close
        );

        let f = fx();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        {
            let mut c = f.ctl.borrow_mut();
            c.model_mut().set_agent(&id, Some(AgentKind::Claude));
            c.set_working(&id, Some(0));
        }
        f.clock.set(5_000);
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::KeepOpen
        );
        assert_eq!(f.log.borrow().raised, 1);
        assert!(f.h.comp.get_dialog_open());
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Stop the agent and close the window?"
        );
        f.h.comp.invoke_dialog_confirm();
        let log = f.log.borrow();
        assert_eq!(log.quit, 1);
        assert!(log.hidden >= 1);
        assert!(f.ctl.borrow().ended);
    }

    #[test]
    fn under_a_question_chords_are_swallowed_except_close_tab() {
        let f = fx();
        let a = f.ctl.borrow_mut().open_folder("C:\\a");
        f.ctl.borrow_mut().open_folder("C:\\b");
        f.ctl
            .borrow_mut()
            .model_mut()
            .set_agent(&a, Some(AgentKind::Claude));
        f.ctl.borrow_mut().request_close(&a);
        assert!(f.h.comp.get_dialog_open());
        assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
        assert_eq!(f.rows(), 2, "Ctrl+T did nothing under the question");
        // Ctrl+W still acts: it closes the tab in front (B has no agent).
        assert_eq!(f.key(ctrl("w", "KeyW")), KeyOutcome::Consumed);
        assert_eq!(f.cwds(), ["C:\\a"]);
    }

    #[test]
    fn the_key_context_reads_the_prefs() {
        let f = fx();
        let c = f.ctl.borrow().key_context();
        assert!(c.agent_keys);
        assert!(!c.vertical);
        set_tab_layout(&f.prefs, TabLayout::Vertical);
        assert!(f.ctl.borrow().key_context().vertical);
        // Ctrl+S is the agent key of the sidebar toggle.
        let before = sidebar_mode(&f.prefs);
        assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Consumed);
        assert_ne!(sidebar_mode(&f.prefs), before);
        set_agent_keys(&f.prefs, false);
        assert!(!f.ctl.borrow().key_context().agent_keys);
        let before = sidebar_mode(&f.prefs);
        f.ctl.borrow_mut().open_folder("C:\\a");
        f.wait_made("C:\\a");
        assert_eq!(f.key(ctrl("s", "KeyS")), KeyOutcome::Pty);
        assert_eq!(sidebar_mode(&f.prefs), before);
    }

    #[test]
    fn a_pick_swaps_the_grid_source_once() {
        let f = fx();
        let a = f.ctl.borrow_mut().open_folder("C:\\a");
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.wait_made("C:\\a");
        f.wait_made("C:\\b");
        assert_eq!(f.ctl.borrow().model().active().unwrap().id, b);
        let resizes: Vec<usize> = f.sp.made().iter().map(|m| m.resize_calls()).collect();
        let sources = f.log.borrow().sources.len();
        // A click (press and release, no travel) on tab A.
        let (x, origin) = {
            let c = f.ctl.borrow();
            let r = &c.view().rows[0];
            (
                (f64::from(r.x) + f64::from(r.w) / 2.0) / c.scale,
                f64::from(0u8),
            )
        };
        f.h.comp
            .invoke_strip_press(a.as_str().into(), 0, x as f32, origin as f32);
        f.h.comp.invoke_strip_released();
        assert_eq!(f.ctl.borrow().model().active().unwrap().id, a);
        assert_eq!(f.log.borrow().sources.len(), sources + 1);
        assert_eq!(
            f.log.borrow().sources.last().unwrap().as_deref(),
            Some(a.as_str())
        );
        let after: Vec<usize> = f.sp.made().iter().map(|m| m.resize_calls()).collect();
        assert_eq!(resizes, after, "a pick does not resize a shell");
        f.ctl.borrow_mut().toggle_settings();
        assert_eq!(f.log.borrow().sources.last().unwrap(), &None);
    }

    #[test]
    fn a_handoff_opens_new_tabs_and_raises() {
        let f = fx_with(Opts {
            real_fs: true,
            ..Opts::default()
        });
        let sub = f.dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap(); // sync-ok: test
        let h = || Handoff {
            cwd: f.dir.path().to_string_lossy().into_owned(),
            args: vec!["flare.exe".to_string(), "sub".to_string()],
        };
        f.start_restore_and_wait();
        f.ctl.borrow_mut().handoff(h());
        f.pump_until("a tab", |f| f.rows() == 1);
        assert_eq!(Path::new(&f.cwds()[0]), sub.as_path());
        assert_eq!(f.log.borrow().raised, 1);
        f.ctl.borrow_mut().handoff(h());
        f.pump_until("a second tab", |f| f.rows() == 2);
    }

    #[test]
    fn nothing_opens_or_raises_after_the_end_of_life() {
        let f = fx_with(Opts {
            real_fs: true,
            ..Opts::default()
        });
        f.start_restore_and_wait();
        f.ctl.borrow_mut().end();
        let dir = f.dir.path().to_string_lossy().into_owned();
        f.ctl.borrow_mut().on_msg(UiMsg::Handoff(Handoff {
            cwd: dir.clone(),
            args: vec!["flare.exe".to_string(), dir.clone()],
        }));
        f.ctl.borrow_mut().on_msg(UiMsg::Folders(vec![dir.clone()]));
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Picked(Some(PathBuf::from(&dir))));
        f.ctl.borrow_mut().on_msg(UiMsg::FolderResolved(dir));
        assert_eq!(f.rows(), 0);
        assert_eq!(f.log.borrow().raised, 0);
    }

    #[test]
    fn a_handoff_before_the_restore_is_queued() {
        let f = fx_with(Opts {
            saved: vec![("C:\\kept", None)],
            ..Opts::default()
        });
        let sub = f.dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap(); // sync-ok: test
        f.ctl.borrow_mut().handoff(Handoff {
            cwd: f.dir.path().to_string_lossy().into_owned(),
            args: vec!["flare.exe".to_string(), "sub".to_string()],
        });
        std::thread::sleep(Duration::from_millis(150));
        f.pump();
        assert_eq!(f.rows(), 1, "only the placeholder: the hand-off is queued");
        f.start_restore_and_wait();
        f.pump_until("the queued tab", |f| f.rows() == 2);
        assert_eq!(f.cwds()[0], "C:\\kept");
        assert_eq!(Path::new(&f.cwds()[1]), sub.as_path());
    }

    fn observed(w: i32, h: i32, scale: f64, maximised: bool) -> Observed {
        Observed {
            width_phys: w,
            height_phys: h,
            x: 100,
            y: 50,
            scale,
            maximised,
            minimised: false,
            fullscreen: false,
        }
    }

    fn window_file(f: &Fx) -> Option<flare_win::window_state::WindowState> {
        let raw = std::fs::read_to_string(f.dir.path().join("window.json")).ok()?; // sync-ok: test
        Some(parse_window_state(
            &raw,
            &[Area {
                x: 0,
                y: 0,
                width: 4000,
                height: 3000,
            }],
        ))
    }

    #[test]
    fn window_geometry_is_saved_after_the_delay_and_on_close() {
        let f = fx();
        f.ctl
            .borrow_mut()
            .on_geometry(observed(1500, 900, 1.5, false));
        assert!(window_file(&f).is_none(), "not yet");
        f.pump_until("window.json", |f| window_file(f).is_some());
        let s = window_file(&f).unwrap();
        assert_eq!((s.width, s.height), (1000, 600));
        assert_eq!((s.x, s.y), (Some(100), Some(50)));
        assert!((s.scale - 1.5).abs() < 1e-9);
        // A maximised observation never overwrites the size.
        f.ctl
            .borrow_mut()
            .on_geometry(observed(3000, 1800, 1.5, true));
        f.pump_until("the maximised flag", |f| {
            window_file(f).is_some_and(|s| s.maximised)
        });
        assert_eq!(window_file(&f).unwrap().width, 1000);
        // The close writes at once.
        f.ctl
            .borrow_mut()
            .on_geometry(observed(1800, 1200, 1.5, false));
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::Close
        );
        let s = window_file(&f).unwrap();
        assert_eq!((s.width, s.height), (1200, 800));
    }

    #[test]
    fn tick_stops_its_timer_when_nothing_is_pending() {
        let f = fx();
        set_tab_layout(&f.prefs, TabLayout::Vertical);
        assert!(!f.ctl.borrow().timer_running());
        f.ctl.borrow_mut().toggle_sidebar();
        assert!(f.ctl.borrow().timer_running());
        f.clock.set(2_000);
        f.ctl.borrow_mut().tick();
        assert!(!f.ctl.borrow().timer_running());
    }

    #[test]
    fn a_reported_unc_folder_is_never_adopted() {
        let f = fx();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        f.wait_made("C:\\a");
        let m = f.made_with("C:\\a").remove(0);
        m.feed(b"\x1b]9;9;\\\\evil\\share\x07");
        m.feed(b"\x1b]9;9;C:\\w\x07");
        f.pump_until("the local report", |f| f.cwds() == ["C:\\w"]);
        assert_eq!(f.ctl.borrow().model().state.tabs[0].id, id);
    }

    #[test]
    fn a_menu_picks_a_recent_folder_and_copies_a_path() {
        let f = fx();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        f.ctl.borrow_mut().open_tab_menu(&id, 10.0, 10.0);
        assert!(f.h.comp.get_menu_open());
        f.h.comp.invoke_menu_picked(1); // Copy folder path
        assert_eq!(f.log.borrow().clipboard, ["C:\\a"]);
        assert!(!f.h.comp.get_menu_open());
        f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        assert!(f.h.comp.get_menu_open());
        f.h.comp.invoke_menu_picked(0); // the recent folder C:\a
        assert_eq!(f.rows(), 2);
    }
}
