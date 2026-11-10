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

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use flare_agent::close::holds_window_close;
use flare_agent::icon_motion::{ICON_STILL, IconPhase, MotionPicks, icon_phase};
use flare_agent::keys::AgentView;
use flare_agent::kinds::{AgentKind, DetectedAgent};
use flare_agent::mark_motion::{MarkPhase, MarksPause, STILL, mark_phase, needs_clock, pulse_at};
use flare_agent::resume::valid_resume;
use flare_agent::reveal::agent_of_resume;
use flare_agent::tab_look::{Switches, TabSets};
use flare_agent::tab_mark::MarkState;
use flare_render::metrics::CellMetrics;
use flare_settings::agent_prefs::{
    agent_done_on, agent_failed_on, agent_guard, agent_question_on, agent_rainbow,
    on_agent_look_change, taskbar_badge_on,
};
use flare_settings::kv::PrefStore;
use flare_settings::kv::Subscription;
use flare_settings::look::LookStore;
use flare_settings::new_tab::new_tab_folder;
use flare_settings::recent_roots::{pinned_roots, recent_roots};
use flare_settings::sidebar::{SidebarMode, sidebar_mode};
use flare_settings::tab_layout::{TabLayout, tab_layout};
use flare_settings::tab_switch::tab_switch;
use flare_settings::term_prefs::agent_keys;
use flare_tabs::close_flow::{CloseFlow, CloseVerdict, Confirmed};
use flare_tabs::decor::{Decors, Refused, Source as DecorSource};
use flare_tabs::drag_flight::DragInFlight;
use flare_tabs::keymap::{ChordId, KeyContext, app_owns_key, key_label};
use flare_tabs::model::TabModel;
use flare_tabs::places::place_rows;
use flare_tabs::restore::{
    Restored, SavedTabs, SessionLookup, is_unsafe_restore_folder, resolve_restore,
    with_launch_folders,
};
use flare_tabs::reveal::Point;
use flare_tabs::tab_drag::Release;
use flare_tabs::tabs::{Pending, TabKind};
use flare_tabs::tabs_store::TabsStore;
use flare_tabs::tip_place::{BoxF, TipSide};
use flare_term::click_caret::MouseTracking;
use flare_term::emulator::TermEvent;
use flare_term::handle::TermHandle;
use flare_term::keys::{KeyEvent, KeyState, term_key_action};
use flare_term::path_open::{PathOpeners, StatFn};
use flare_theme::agent_colours::theme_agent_colours;
use flare_theme::look::apply_look;
use flare_theme::resolve::preset_accent;
use flare_win::argv::{folders_from_argv, own_files};
use flare_win::clipboard_port::ClipboardPort;
use flare_win::dblclick::DoubleClick;
use flare_win::single_instance::Handoff;
use flare_win::taskbar::TaskbarBadge;
use flare_win::window_state::{NormalBounds, Observed, WindowStateStore};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::agent_hub::{AgentHub, HubDeps, e2e_sonara_post, sonara_post};
use crate::chords::{self, ChordTarget, Dispatch};
use crate::convo_ctl::{ConvoCtl, ConvoMsg, ConvoMsgBox, Want};
use crate::ctl_app::{CtlOp, CtlRequestBox};
use crate::font_ctl::{FontCtl, effective_px};
use crate::help_ctl::{HelpCtl, next_open_seq};
use crate::mark_colours::{UiMarks, current_palette, indicator_of, picks_from, ui_marks};
use crate::menus_ui::{
    MenuKind, MenuRowData, OverflowRowData, TipHost, glyph_path, overflow_rows, plus_menu_rows,
    tab_menu_rows, toggle_pin_in_menu,
};
use crate::panel_ctl::{PanelCtl, PanelMsgBox};
use crate::settings_ctl::{SettingsCtl, SettingsMsg};
use crate::side_slide::{SideSlide, SlideAim};
use crate::sidebar_ui::{SidebarCtl, SidebarEvent};
use crate::startup::{Ask, PendingFolders, ask_view, end_of_life, tab_ask, window_ask};
use crate::tab_host::{HostEvent, TabHost};
use crate::tabs_ui::{
    AgentRows, GeomData, RowData, SideDrag, StripDrag, View, ViewInput, build_view, needs_of,
    slot_to_model, sync_rows,
};
use crate::term_menu::to_menu_row;
use crate::term_ui::{
    CursorKind, GridGeom, KeyDone, PointerIn, SetThumb, TabInfo, TermDeps, TermMsg, TermUi,
};
use crate::thumbs::{ThumbView, fit_pic, thumb_spot};
use crate::update_ctl::{AskInstall, Spawn, UpdateCtl, UpdateDeps, UpdateMsg};
use crate::web_ctl::{WebCtl, WebMsgBox};
use crate::{
    AppWindow, HelpBridge, MarkMotion, MenuRow, OverflowRow, Place, SettingsBridge, StripGeom,
    TabRow,
};
use flare_theme::fonts::zoom_step;

/// Everything a worker thread can say to the UI thread. WHY a message and not
/// `slint::invoke_from_event_loop` closures: the headless platform has no event loop proxy, and an
/// `Rc<RefCell<AppCtl>>` is not `Send`. Production posts with `invoke_from_event_loop` into a
/// thread-local that holds the `AppCtl` and calls `on_msg` (Task 30b); tests post into a queue and
/// call `pump()`.
#[derive(Debug)]
pub enum UiMsg {
    RestoreDone(Restored),
    Picked(Option<PathBuf>),
    /// The starter folders of the folder menu (Documents, Downloads, Desktop, the user folder: those that exist),
    /// resolved by a worker.
    StarterFolders(Vec<String>),
    /// A dropped path, resolved by a worker to the folder it names (or holds).
    FolderResolved(String),
    /// The folders a hand-off's arguments named, resolved by a worker.
    Folders(Vec<String>),
    Handoff(Handoff),
    HostWake,
    /// A worker's answer for the terminal controller (clipboard read, copy result, path stats).
    Term(TermMsgBox),
    /// The theme, the indicator, a colour pick or an attention switch changed.
    LookChanged,
    /// A setting the tab view reads changed (the tab position, the keys, the taskbar count...): the
    /// view refreshes at once, Settings open or not (owner bug 2026-10-21). One per turn.
    PrefsChanged,
    /// A worker's answer or a setting change for the Settings controller (Task 41).
    Settings(SettingsMsg),
    /// A command help setting changed (the switch, the shell chip or the panel width).
    Help,
    /// A worker's answer or a timer for the update controller (M9 Task 30).
    Update(UpdateMsg),
    /// A worker's answer for the panel controller (Surfaces stage 1): a picture or a text file read.
    Panel(PanelMsgBox),
    /// A call of the control channel that needs the window or a tab's panel (Surfaces stage 2). The
    /// worker waits on the request's reply channel, so this is answered at once, never queued behind
    /// anything slow.
    Ctl(CtlRequestBox),
    /// The web panel's browser told the window something, or a still was decoded (Surfaces stage 3).
    Web(WebMsgBox),
    /// A transcript tracker's snapshot, or a path clicked in a diff (Surfaces stage 6).
    Convo(ConvoMsgBox),
    /// The window lost the keyboard while it was still the foreground window: asks again once the
    /// switch has settled (it may be the keyboard moving into the page, or a real switch to another app).
    FocusLostCheck,
    /// The app link the person allowed did not open (the launcher's worker answered, rule 252).
    AppLinkFailed {
        tab: String,
        scheme: String,
        error: String,
    },
    /// A worker looked whether anything listens on a local port a tab printed (Surfaces stage 7).
    DevLook {
        tab: String,
        token: u64,
        look: flare_surfaces::devpreview::PortLook,
    },
    /// The look made when the chip was clicked (the page opens only when it finds the tab's own server).
    DevClickLook {
        tab: String,
        token: u64,
        look: flare_surfaces::devpreview::PortLook,
    },
    /// The person clicked a notification about this tab.
    ToastClicked(String),
    /// Windows would not take a notification.
    ToastFailed,
    /// A worker's answer or a Settings action for the mods controller (Surfaces stage 8).
    Mods(crate::mods_ctl::ModsMsgBox),
    /// The staged build (install when idle, 2026-10-29): a look, a swap, a go-ahead.
    Staged(staged_glue::StagedMsg),
}

mod ask_glue;
mod bar_glue;
mod install_glue;
mod mods_glue;
mod plugins_glue;
mod sessions_glue;
pub mod staged_glue;
mod surfaces7;
mod web_glue;

pub use surfaces7::install as install_surfaces7;

/// Publishes a control channel event: name, params, and the tab it is about (its connections only).
pub type CtlEvents = Arc<dyn Fn(&str, serde_json::Value, Option<&str>) + Send + Sync>;

/// What the control channel looks like from outside, for the e2e probe and the diagnostics.
#[derive(Clone, Debug, Default)]
pub struct CtlInfo {
    pub pipe: String,
    pub mcp_exe: Option<String>,
}

/// A `TermMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never the
/// clipboard text.
pub struct TermMsgBox(pub TermMsg);

impl std::fmt::Debug for TermMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            TermMsg::Clip(..) => "TermMsg::Clip",
            TermMsg::Copied(_) => "TermMsg::Copied",
            TermMsg::BigCopied(..) => "TermMsg::BigCopied",
            TermMsg::PathAnswer(..) => "TermMsg::PathAnswer",
            TermMsg::Preview { .. } => "TermMsg::Preview",
            TermMsg::PreviewRefused { .. } => "TermMsg::PreviewRefused",
            TermMsg::Thumb { .. } => "TermMsg::Thumb",
        })
    }
}

pub type Post = Arc<dyn Fn(UiMsg) + Send + Sync>;
type PickAnswer = Arc<dyn Fn(Option<PathBuf>) + Send + Sync>;
type StarterAnswer = Arc<dyn Fn(Vec<String>) + Send + Sync>;

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
    /// The folder menu's starter folders. Production: `flare_win::paths::starter_folders` on a worker, which then
    /// answers; tests: a fake that never answers (no starters), or one that does.
    pub starter_folders: Rc<dyn Fn(StarterAnswer)>,
    pub clipboard: Rc<dyn Fn(&str)>,
    pub open_url: Rc<dyn Fn(&str)>,
    /// Production: `GridHost::set_source(Box::new(LiveSource::new(h)))`; tests: records the id.
    pub set_grid_source: Rc<dyn Fn(Option<Arc<TermHandle>>)>,
    pub raise: Rc<dyn Fn()>,
    pub quit: Rc<dyn Fn()>,
    pub hide: Rc<dyn Fn()>,
    pub launch_folders: Vec<String>,
    pub e2e: bool,
    /// The terminal's ports (M5): the real ones in production, in-memory stand-ins under `--e2e`.
    pub term: TermPorts,
}

/// What the update controller needs from the glue (M9 Task 30; Task 32 builds the engine).
pub struct UpdateParts {
    pub engine: Arc<flare_update::engine::Engine>,
    pub current_version: String,
    pub e2e_dir: Option<PathBuf>,
    pub spawn: Spawn,
}

/// What the terminal controller needs from the outside (Task 24).
pub struct TermPorts {
    /// Production `WinClipboard`; `MemoryClipboard` under `--e2e`.
    pub clip_port: Arc<dyn ClipboardPort>,
    /// Production `WinOpeners`; `RecordingOpeners` under `--e2e`.
    pub openers: Arc<dyn PathOpeners + Send + Sync>,
    /// The agent view the keyboard path asks about. None: the hub's arm state (production). A test
    /// may pass its own.
    pub agent: Option<Rc<dyn AgentView>>,
    pub dbl: DoubleClick,
    /// The stat behind the path-link questions (`FsStat` in production).
    pub stat: Arc<dyn StatFn>,
    /// The grid's cell metrics now (production: the grid host's).
    pub metrics: Rc<dyn Fn() -> CellMetrics>,
    /// Marks the grid dirty so the next frame redraws the decoration.
    pub mark_grid_dirty: Rc<dyn Fn()>,
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
/// The slide's fallback step when no frame comes.
/// How long the page's width must hold still before the text view is wrapped to it (a window edge being dragged).
const WRAP_SETTLE_MS: u64 = 120;

const SLIDE_STEP_MS: u64 = 16;
/// The mark clock's frame (about 60 per second); it runs only while a mark moves.
const MARK_FRAME_MS: u64 = 16;

/// The one clock behind every moving mark, the skeleton's pulse included. The timer runs only
/// while something moves, a window is showing and Windows' animation effects are on.
struct MarkClock {
    timer: slint::Timer,
    /// Shared with the timer's body: a minimised window moves no mark.
    pause: Rc<RefCell<MarksPause>>,
    /// The pause clock's reading when the marks started moving; `None` while nothing moves, so a
    /// restart after a pure stop begins at phase zero and a restore after minimising goes on.
    start: Option<u64>,
}

impl MarkClock {
    fn new() -> Self {
        Self {
            timer: slint::Timer::default(),
            pause: Rc::new(RefCell::new(MarksPause::new())),
            start: None,
        }
    }
}

/// A row's mark state integer back to the state (0 is none).
fn mark_state_of(n: i32) -> Option<MarkState> {
    match n {
        1 => Some(MarkState::Working),
        2 => Some(MarkState::Question),
        3 => Some(MarkState::Done),
        4 => Some(MarkState::Failed),
        _ => None,
    }
}

/// Puts one instant of every moving mark, and of every agent icon's motion, on the window.
fn push_frame(app: &AppWindow, p: MarkPhase, icon: &IconPhase, pulse: f32) {
    use slint::Model as _;
    let g = app.global::<MarkMotion>();
    g.set_still(p.still);
    g.set_spin(p.spin_deg);
    g.set_breathe(p.breathe);
    g.set_run(p.run_t);
    g.set_flow(p.flow_px);
    g.set_pulse(pulse);
    g.set_hop_y(icon.hop_y);
    g.set_hop_sx(icon.hop_sx);
    g.set_hop_sy(icon.hop_sy);
    g.set_blink(icon.blink);
    g.set_spin_fade(icon.spin_fade_deg);
    g.set_fade(icon.fade);
    g.set_turn(icon.spin_deg);
    g.set_grow(icon.grow);
    g.set_grow_alpha(icon.grow_alpha);
    // agy's arch: one colour a path, written into the model in place once it holds them (no new model a frame).
    let flow = g.get_agy_flow();
    let colours = icon
        .flow
        .iter()
        .map(|&(r, gr, b)| slint::Color::from_rgb_u8(r, gr, b));
    if flow.row_count() == icon.flow.len() {
        for (i, c) in colours.enumerate() {
            if flow.row_data(i) != Some(c) {
                flow.set_row_data(i, c);
            }
        }
    } else {
        g.set_agy_flow(ModelRc::new(VecModel::from(colours.collect::<Vec<_>>())));
    }
}

/// The marks the look and the picks give now.
fn compute_marks(prefs: &PrefStore) -> UiMarks {
    let store = LookStore::new(prefs.clone());
    let snap = store.input_snapshot();
    let look = apply_look(&snap.as_input(false));
    let accent = preset_accent(
        &look.theme_id,
        &look.theme_id,
        snap.custom.as_ref(),
        snap.draft.as_ref(),
    );
    let themed = theme_agent_colours(
        &look.term,
        accent.as_deref(),
        snap.background.as_deref(),
        snap.accent.as_deref(),
    );
    let rainbow = agent_rainbow(prefs);
    let text = look
        .chrome
        .vars
        .get("--p-text")
        .cloned()
        .unwrap_or_default();
    ui_marks(
        &current_palette(&look, &themed, &picks_from(&store), rainbow),
        rainbow,
        &text,
    )
}

/// The attention switches and the indicator style now.
/// Replays a right click at `(x, y)` through Slint's own event path (move, press, release) from a zero
/// timer, so the surface under a scrim that swallowed the press decides what it means.
fn hand_on_right_click(weak: slint::Weak<AppWindow>, x: f32, y: f32) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(app) = weak.upgrade() else { return };
        let position = slint::LogicalPosition::new(x, y);
        let win = app.window();
        let button = slint::platform::PointerEventButton::Right;
        win.dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        win.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        win.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    });
}

fn switches_of(prefs: &PrefStore) -> Switches {
    Switches {
        indicator: indicator_of(&LookStore::new(prefs.clone())),
        done_on: agent_done_on(prefs),
        question_on: agent_question_on(prefs),
        failed_on: agent_failed_on(prefs),
        rainbow: agent_rainbow(prefs),
    }
}

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
    /// How many times the window told the controller the grid area changed (the e2e holds that a splitter drag
    /// makes none and its release one).
    grid_fits: u64,
    scale: f64,
    timer: slint::Timer,
    timer_on: bool,
    /// The sidebar's slide, eased frame by frame from its first frame (audit V3).
    side_slide: SideSlide,
    /// Steps the slide where no OS window draws frames (the tests, the scenes).
    slide_timer: slint::Timer,
    /// The slide's clock in fractional ms; the window sets the precise one.
    frame_clock: Rc<dyn Fn() -> f64>,
    /// The e2e holds the slide at an offset for a picture: no frame moves it until released.
    slide_held: bool,
    rows_model: Rc<VecModel<TabRow>>,
    /// The plugins' section rows and top-bar buttons (plugins phase 4), each one model kept for the window's life.
    plugin_rows_model: Rc<VecModel<crate::PluginRow>>,
    plugin_top_model: Rc<VecModel<crate::PluginButton>>,
    /// The bars the tabs show (plugins phase 6), the front bar's chips as Slint draws them and as they were built.
    bars: crate::plugin_bar::Bars,
    bar_chips_model: Rc<VecModel<crate::PluginButton>>,
    bar_chips: Vec<crate::plugin_ui::Button>,
    /// The plugins' sidebar sections and actions (plugins phase 4).
    plugins: plugins_glue::PluginSurfaces,
    /// The transcripts followed for the plugins that listen to `session`, and each plugin's share (plugins phase 5).
    sessions: crate::plugin_sessions::PluginSessions,
    view: View,
    last_geom: Option<GeomData>,
    query: String,
    boot_saved: SavedTabs,
    restore_started: bool,
    /// The folder menu's starter folders, once a worker has found them (empty until then).
    starters: Vec<String>,
    source_id: Option<String>,
    last_saved: Option<SavedTabs>,
    /// Agents that are mid-answer: tab id to the clock time they started (M6 fills this).
    working: HashMap<String, u64>,
    ended: bool,
    /// Stops the hand-off server (set by the window); run once at the start of the end of life.
    handoff_stop: Option<Rc<dyn Fn()>>,
    /// The grid's face and size (Task 27); set by the window after the controller is made.
    font: Option<FontCtl>,
    /// The terminal's input controller: selection, copy, paste, find, links and the menu (M5).
    term: TermUi,
    /// When the terminal next wants the timer (None: idle).
    term_due: Option<u64>,
    /// Timer rounds still owed after output arrived: path questions are asked by a drawn frame.
    term_hot: u8,
    /// The find bar was shown at the last push (focus follows it opening and closing).
    find_shown: bool,
    /// The agent indicator, the arm state, the poll and the question watches (Task 28).
    hub: AgentHub,
    /// The dictation key path (M8): offered every key first.
    dictation: crate::dictation_ctl::DictationKeys,
    /// The tabs the hub has been given a handle for; one that goes away is forgotten.
    hub_known: HashSet<String>,
    /// The badges and progress callers put on tabs (plugins phase 1): the agent's own, through the channel.
    decor: Decors,
    /// How each agent's icon moves while it works: the plugins' picks (`agents.motion.*`, owner 2026-10-26).
    pub(crate) motions: MotionPicks,
    /// The tabs as the plugins last heard of them (plugins phase 3): the next refresh tells them what changed.
    /// None while no page listens.
    plugin_seen: Option<Vec<crate::appctl::mods_glue::TabFact>>,
    /// The hub's view of the host's handles, refreshed before every hub call.
    handles: Rc<RefCell<HashMap<String, Arc<TermHandle>>>>,
    /// The colours the marks wear, from the look and the picks.
    marks: UiMarks,
    /// The rainbow switch `marks` was made with.
    marks_rainbow: bool,
    _look_subs: Vec<Subscription>,
    /// The listeners of every other key a Settings row stores, and the flag that makes many
    /// changes in one turn one `PrefsChanged`.
    _prefs_subs: Vec<Subscription>,
    prefs_dirty: Arc<AtomicBool>,
    /// The mark clock (Task 29).
    clock: MarkClock,
    /// Windows' "animation effects" switch; tests inject it.
    animations: Rc<dyn Fn() -> bool>,
    /// The window is minimised: no mark moves.
    minimised: bool,
    /// The resume skeleton shows over the front tab (it pulses on the mark clock).
    skeleton: Rc<Cell<bool>>,
    /// Where the skeleton is in its life (shown, fading, gone).
    skel: Skel,
    /// The skeleton has seen a real resume (so it fades instead of vanishing).
    skel_resumed: bool,
    /// The arm state as the keyboard path sees it (the hub's, unless a test passed its own).
    agent_view: Rc<dyn AgentView>,
    /// The window has the keyboard focus (a look needs it).
    focused: bool,
    /// Asks the taskbar to flash its button (a bell rang). The window decides whether anyone is
    /// looking: the real one flashes only while the window is not in front.
    flash: Rc<dyn Fn()>,
    /// What the hub was last told is being looked at.
    looked: Option<(Option<String>, bool)>,
    /// The taskbar overlay, made once the window has a handle.
    badge: Option<TaskbarBadge>,
    /// The badge's last outcome (count, on, drawn), so the log hears each change once.
    badge_seen: Option<(usize, bool, bool)>,
    /// Tabs restored with an agent to resume: the kind and when they were spawned. They wear the
    /// agent's icon from the first frame and are saved as that agent until the hub takes over.
    restoring: HashMap<String, (AgentKind, u64)>,
    /// Tabs whose panel their agent opened and where the hub has seen that agent since: when it is no longer
    /// there, the panel closes (owner, 2026-10-24: a panel Claude opened belongs to its session, not the tab).
    agent_panel_seen: HashSet<String>,
    /// While a page in the panel holds the window full screen: whether the window was full screen before it.
    web_full_was: Option<bool>,
    /// The Settings controller (Task 41); the window hands it over after the controller exists.
    settings: Option<Rc<RefCell<SettingsCtl>>>,
    /// The Settings page is in front now.
    settings_shown: bool,
    /// A Settings tab exists.
    settings_present: bool,
    /// The Command help controller (Task 43), the front shell it opens on, and its settings watch.
    help: Option<Rc<RefCell<HelpCtl>>>,
    help_front: Rc<RefCell<Option<String>>>,
    help_dirty: Arc<AtomicBool>,
    _help_sub: Option<Subscription>,
    /// The update controller (M9 Task 30); the window glue hands it over after this one exists.
    update: Option<Rc<RefCell<UpdateCtl>>>,
    /// The update install is waiting on the close question: the answer goes here.
    install_cb: Option<Box<dyn FnOnce(bool)>>,
    /// The engine's worker sets this once the installer is started: the next close goes through.
    update_agreed: Arc<AtomicBool>,
    /// The update window was put away for a close question: it comes back when that is cancelled.
    update_stepped_aside: bool,
    /// The next close question was opened by a chord (the ring shows from the start).
    ask_by_key: bool,
    /// The tab in front when the last `tab-switch` crumb was said.
    crumbed_front: Option<String>,
    /// When each tab's shell was asked for, its folder and whether it resumes: the `shell-*` crumbs.
    spawn_marks: HashMap<String, (std::time::Instant, String, bool)>,
    /// The Preview chip and the question notifications (Surfaces stage 7).
    s7: surfaces7::Surf7,
    /// Mods (Surfaces stage 8): none until the window hands over the hub.
    mods: Option<crate::mods_ctl::ModCtl>,
    /// The tabs' panels (Surfaces stage 1).
    panels: PanelCtl,
    /// The conversation panels' transcript trackers (Surfaces stage 6; none run while no such panel is open).
    convo: ConvoCtl,
    /// The web panel's browser (made by the first web panel; nothing exists while none is open).
    web: WebCtl,
    /// The downloads of web pages waiting for a yes (S6).
    downloads: crate::download_ask::DownloadQueue,
    /// The app links of web pages waiting for the person's Open (rule 252).
    app_asks: crate::app_ask::AppAsks,
    /// What opens an app link after Open: a recorder until startup hands in the shell (rule 252).
    app_launcher: Rc<dyn crate::app_ask::AppLauncher>,
    /// A plugin's question in Flare's own dialog (a fill to approve, a secret to type), one at a time (the `fill`
    /// permission, 2026-10-28).
    plugin_ask: Option<ask_glue::PluginAsk>,
    /// Numbers the plugins' questions, so a timer of an older one ends nothing.
    plugin_ask_seq: u64,
    /// The panel had the keys at the last look, so they go back to the terminal when it goes away.
    panel_focus_seen: bool,
    /// The tab that was in front at the last panel sync: another one letting go of a splitter drag.
    panel_front_seen: Option<String>,
    /// Where control channel events go (None: no channel, in tests and headless runs).
    ctl_events: Option<CtlEvents>,
    ctl_info: Option<CtlInfo>,
    /// The staged build of an installed copy (install when idle); None for any other run.
    staged: Option<staged_glue::StagedState>,
}

/// The row the terminal's menu gains on a tab that runs Claude (Surfaces stage 6; reworded 2026-10-22 so the
/// owner needs no explanation; the task list row was removed 2026-10-26). The row shows only on a tab that is
/// Claude (a session or the detection), so it always names Claude. "Hide ..." is the same words while the panel
/// is open.
const CONVO_SHOW_CHANGES: &str = "Show files Claude changed";
const CONVO_HIDE_CHANGES: &str = "Hide files Claude changed";
/// The terminal menu's row that sends the grid selection to the tab's agent (#33).
const ASK_AGENT_ROW: &str = "Ask agent about this selection";

/// A row about the tab's agent in the terminal's menu: its own group, after a rule, and nothing to resolve when picked.
fn agent_menu_row(label: &str, icon: &'static str) -> (MenuRowData, String) {
    (
        MenuRowData {
            label: label.to_string(),
            hint: String::new(),
            icon,
            filled_icon: false,
            disabled: false,
            trailing_icon: None,
            trailing_filled: false,
            trailing_tip: String::new(),
            group: true,
            flyout: false,
            parent: -1,
            icon_cmd: String::new(),
            logo: None,
        },
        String::new(),
    )
}

/// How long the skeleton fades after the agent has painted (PT `ResumeSkeleton`).
const SKELETON_FADE_MS: u64 = 180;
/// A restored tab keeps its saved agent this long without the hub having seen it.
const RESTORE_GRACE_MS: u64 = 15_000;

/// The resume skeleton over the front tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Skel {
    Gone,
    Shown,
    /// Fading; gone at the moment given.
    Leaving(u64),
}

/// The front shell tab as owned pieces; `info` lends them to the terminal controller.
struct FrontTab {
    id: String,
    handle: Arc<TermHandle>,
    cwd: Option<String>,
    alive: bool,
}

impl FrontTab {
    fn info(&self) -> TabInfo<'_> {
        TabInfo {
            id: &self.id,
            handle: &self.handle,
            cwd: self.cwd.as_deref(),
            alive: self.alive,
        }
    }
}

impl TermPorts {
    /// Stand-ins that touch nothing outside the process: tests and the headless scenes. The
    /// metrics are an 8 by 16 cell at scale 1.
    pub fn inert() -> Self {
        use flare_render::glyph::{FakeGlyphs, GlyphSource};
        Self {
            clip_port: Arc::new(flare_win::clipboard_port::MemoryClipboard::new()),
            openers: Arc::new(crate::openers::RecordingOpeners::new()),
            agent: None,
            dbl: flare_win::dblclick::FALLBACK,
            stat: Arc::new(flare_term::path_open::FsStat),
            metrics: Rc::new(|| {
                flare_render::metrics::cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0)
            }),
            mark_grid_dirty: Rc::new(|| {}),
        }
    }
}

/// The clipboard, the openers and the url opener of a run: the real ones, or under `--e2e` the
/// in-memory stand-ins that never touch the desktop (parity: under the test flag nothing opens
/// outside the app).
pub struct Backends {
    pub clip_port: Arc<dyn ClipboardPort>,
    pub openers: Arc<dyn PathOpeners + Send + Sync>,
    /// Under `--e2e`: what the openers were asked to do.
    pub recording: Option<Arc<crate::openers::RecordingOpeners>>,
    /// Under `--e2e`: the urls that would have been opened.
    pub urls: Option<Arc<std::sync::Mutex<Vec<String>>>>,
}

impl Backends {
    pub fn new(e2e: bool) -> Self {
        if e2e {
            let rec = Arc::new(crate::openers::RecordingOpeners::new());
            Self {
                clip_port: Arc::new(flare_win::clipboard_port::MemoryClipboard::new()),
                openers: rec.clone(),
                recording: Some(rec),
                urls: Some(Arc::default()),
            }
        } else {
            Self {
                clip_port: Arc::new(flare_win::clipboard_port::WinClipboard),
                openers: Arc::new(crate::openers::WinOpeners),
                recording: None,
                urls: None,
            }
        }
    }

    /// Opens a web address, or under `--e2e` only records it. The caller has checked `is_web_url`;
    /// `shell_open::open_url` refuses anything but http(s) again.
    pub fn open_url(&self) -> Rc<dyn Fn(&str)> {
        match &self.urls {
            Some(log) => {
                let log = Arc::clone(log);
                Rc::new(move |url: &str| {
                    log.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(url.to_string());
                    // The test hook's url.log probe answers from its own list: both see the open.
                    crate::e2e_probes::input_b::record_url(url);
                })
            }
            None => Rc::new(|url: &str| {
                if let Err(e) = flare_win::shell_open::open_url(url) {
                    log::warn!("open {url} failed: {e}");
                }
            }),
        }
    }
}

/// Puts the hover thumbnail on the window, or takes it down. Measured with its picture in place, then placed above or
/// below the path; in a window too small for that it is not shown at all, because it must never cover the path.
fn show_thumb(app: &AppWindow, view: Option<&ThumbView>) {
    let Some(v) = view else {
        app.set_thumb_visible(false);
        return;
    };
    let t = &v.thumb;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(t.width, t.height);
    buf.make_mut_bytes().copy_from_slice(&t.rgba);
    let pic = (f64::from(t.width), f64::from(t.height));
    app.set_thumb_picture(slint::Image::from_rgba8(buf));
    app.set_thumb_caption(t.caption().into());
    app.set_thumb_pic_w(pic.0 as f32);
    app.set_thumb_pic_h(pic.1 as f32);
    let scale = f64::from(app.get_scale());
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let win = app.window().size();
    // Kept left of the grid's right edge: a web panel is a native window and would cover the bubble.
    let window = (
        (f64::from(win.width) / scale).min(f64::from(app.get_grid_right())),
        f64::from(win.height) / scale,
    );
    let item = BoxF {
        left: v.item.left + f64::from(app.get_grid_left()),
        top: v.item.top + f64::from(app.get_grid_top()),
        ..v.item
    };
    let chrome = (
        f64::from(app.get_thumb_w()) - pic.0,
        f64::from(app.get_thumb_h()) - pic.1,
    );
    let Some((w, h)) = fit_pic(pic, chrome, item, window) else {
        app.set_thumb_visible(false);
        return;
    };
    app.set_thumb_pic_w(w as f32);
    app.set_thumb_pic_h(h as f32);
    let size = (f64::from(app.get_thumb_w()), f64::from(app.get_thumb_h()));
    let Some(spot) = thumb_spot(item, size, window) else {
        app.set_thumb_visible(false);
        return;
    };
    app.set_thumb_x(spot.x as f32);
    app.set_thumb_y(spot.y as f32);
    app.set_thumb_visible(true);
}

fn cursor_of(k: CursorKind) -> slint::private_unstable_api::re_exports::MouseCursorInner {
    use slint::private_unstable_api::re_exports::{BuiltInMouseCursor, MouseCursorInner};
    MouseCursorInner::BuiltIn(match k {
        CursorKind::Text => BuiltInMouseCursor::Text,
        CursorKind::Pointer => BuiltInMouseCursor::Pointer,
        CursorKind::Default => BuiltInMouseCursor::Default,
    })
}

/// Binds one callback of the window to a method of the controller.
macro_rules! bind {
    ($app:expr, $rc:expr, $reg:ident, |$c:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
        let weak = Rc::downgrade($rc);
        $app.$reg(move |$($a: $t),*| {
            if let Some(rc) = weak.upgrade() {
                if let Ok(mut $c) = rc.try_borrow_mut() {
                    crate::diag_ctl::timed(stringify!($reg), || {
                        $body;
                    });
                }
            }
        });
    }};
}

fn menu_row(r: &MenuRowData) -> MenuRow {
    MenuRow {
        label: r.label.as_str().into(),
        hint: r.hint.as_str().into(),
        icon: if r.icon_cmd.is_empty() {
            glyph_path(r.icon).into()
        } else {
            r.icon_cmd.as_str().into()
        },
        filled_icon: r.filled_icon,
        disabled: r.disabled,
        trailing_icon: r.trailing_icon.map_or("", glyph_path).into(),
        trailing_filled: r.trailing_filled,
        trailing_tip: r.trailing_tip.as_str().into(),
        keep_open: false,
        group: r.group,
        flyout: r.flyout,
        flyout_pick: false,
        parent: r.parent,
        logo: r.logo.as_ref().map_or("", |(p, _)| p.as_str()).into(),
        logo_fill: r
            .logo
            .as_ref()
            .map(|(_, c)| crate::plugin_look::brand_colours(c).0)
            .unwrap_or_default(),
    }
}

fn overflow_row(r: &OverflowRowData, rows: &[RowData]) -> OverflowRow {
    let row = rows.iter().find(|x| x.id == r.id);
    OverflowRow {
        id: r.id.as_str().into(),
        label: r.label.as_str().into(),
        folder: r.folder.as_str().into(),
        needs: r.needs,
        loading: r.loading,
        settings: r.settings,
        mark_state: row.map_or(0, |x| x.mark_state),
        mark_color: row.map_or(slint::Color::from_argb_u8(0, 0, 0, 0), |x| x.mark_color),
        agent: row.map_or(0, |x| x.agent),
    }
}

fn detected(kind: Option<AgentKind>) -> DetectedAgent {
    kind.map_or(DetectedAgent::Other, DetectedAgent::from)
}

impl AppCtl {
    /// Binds EVERY callback of the tab window's contract, boots the model from `tabs_store.load()`
    /// (sync-ok: startup), draws the placeholders, and returns. The restore starts with
    /// [`AppCtl::start_restore`].
    pub fn new(app: &AppWindow, deps: Deps) -> Rc<RefCell<AppCtl>> {
        let prefs = deps.prefs.clone();
        let now = (deps.clock)();
        let boot_saved = deps.tabs_store.load(); // sync-ok: startup read of tabs.json
        let mut model = TabModel::boot(&boot_saved, now);
        // A placeholder comes back to the agent it was saved with: its icon and ring from the first
        // frame. (The placeholders are in the saved order.)
        let slot_ids: Vec<String> = model.state.tabs.iter().map(|t| t.id.clone()).collect();
        for (id, saved) in slot_ids.iter().zip(&boot_saved.tabs) {
            if saved.agent.is_some() {
                model.set_agent(id, saved.agent);
            }
        }
        let rows_model: Rc<VecModel<TabRow>> = Rc::new(VecModel::default());
        app.set_tab_rows(ModelRc::from(rows_model.clone()));
        let plugin_rows_model: Rc<VecModel<crate::PluginRow>> = Rc::new(VecModel::default());
        app.set_plugin_rows(ModelRc::from(plugin_rows_model.clone()));
        let plugin_top_model: Rc<VecModel<crate::PluginButton>> = Rc::new(VecModel::default());
        app.set_plugin_top_buttons(ModelRc::from(plugin_top_model.clone()));
        let bar_chips_model: Rc<VecModel<crate::PluginButton>> = Rc::new(VecModel::default());
        app.set_bar_chips(ModelRc::from(bar_chips_model.clone()));
        let handles: Rc<RefCell<HashMap<String, Arc<TermHandle>>>> = Rc::default();
        {
            // The look controller reaches every live tab (front and background) through this.
            let live = Rc::clone(&handles);
            crate::look_ctl::LookCtl::set_tab_source(Rc::new(move || {
                live.borrow()
                    .values()
                    .map(|h| Rc::new(Arc::clone(h)) as Rc<dyn crate::look_ctl::ThemePushTarget>)
                    .collect()
            }));
        }
        let hub = {
            let mgr = deps.host.manager();
            let by_id = Rc::clone(&handles);
            let post = Arc::clone(&deps.post);
            AgentHub::new(HubDeps {
                prefs: prefs.clone(),
                now: Rc::clone(&deps.clock),
                handle: Rc::new(move |id| by_id.borrow().get(id).cloned()),
                token: Rc::new(move |id| mgr.tab_token(id)),
                // Under --e2e the post reads only a SONARA_HOME (the test's sandbox), never the owner's Sonara.
                post_question: if deps.e2e {
                    e2e_sonara_post()
                } else {
                    sonara_post()
                },
                diag: crate::diag_ctl::agent_sink(),
                wake: Arc::new(move || post(UiMsg::HostWake)),
                reports_off: Rc::new(|_| {}),
            })
        };
        let agent_view: Rc<dyn AgentView> = deps.term.agent.clone().unwrap_or_else(|| hub.view());
        let term = Self::make_term(app, &deps, Rc::clone(&agent_view));
        let marks = compute_marks(&prefs);
        let marks_rainbow = agent_rainbow(&prefs);
        let look_subs = {
            let on_change = {
                let post = Arc::clone(&deps.post);
                move || post(UiMsg::LookChanged)
            };
            vec![
                LookStore::new(prefs.clone()).on_look_change(on_change.clone()),
                on_agent_look_change(&prefs, on_change),
            ]
        };
        let prefs_dirty = Arc::new(AtomicBool::new(false));
        let prefs_subs = Self::listen_to_rows(&prefs, &deps.post, &prefs_dirty);
        let panels = PanelCtl::new(Arc::clone(&deps.post));
        let convo = ConvoCtl::new(Arc::clone(&deps.post));
        let web = WebCtl::new(Arc::clone(&deps.post));
        Self::push_rainbow(app, &marks);
        let frame_clock: Rc<dyn Fn() -> f64> = {
            let clock = Rc::clone(&deps.clock);
            Rc::new(move || clock() as f64)
        };
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
            grid_fits: 0,
            scale: 1.0,
            timer: slint::Timer::default(),
            timer_on: false,
            side_slide: SideSlide::default(),
            slide_timer: slint::Timer::default(),
            frame_clock,
            slide_held: false,
            rows_model,
            plugin_rows_model,
            plugin_top_model,
            bars: crate::plugin_bar::Bars::default(),
            bar_chips_model,
            bar_chips: Vec::new(),
            plugins: plugins_glue::PluginSurfaces::default(),
            sessions: crate::plugin_sessions::PluginSessions::default(),
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
            starters: Vec::new(),
            source_id: None,
            last_saved: None,
            working: HashMap::new(),
            ended: false,
            handoff_stop: None,
            font: None,
            term,
            term_due: None,
            term_hot: 0,
            find_shown: false,
            hub,
            hub_known: HashSet::new(),
            decor: Decors::default(),
            motions: MotionPicks::new(),
            plugin_seen: None,
            handles,
            marks,
            marks_rainbow,
            _look_subs: look_subs,
            _prefs_subs: prefs_subs,
            prefs_dirty,
            clock: MarkClock::new(),
            animations: Rc::new(crate::e2e::animations_enabled),
            minimised: false,
            skeleton: Rc::new(Cell::new(false)),
            skel: Skel::Gone,
            skel_resumed: false,
            agent_view,
            focused: true,
            dictation: crate::dictation_ctl::DictationKeys,
            flash: Rc::new(|| {}),
            looked: None,
            badge: None,
            badge_seen: None,
            restoring: HashMap::new(),
            agent_panel_seen: HashSet::new(),
            web_full_was: None,
            settings: None,
            settings_shown: false,
            settings_present: false,
            help: None,
            help_front: Rc::default(),
            help_dirty: Arc::new(AtomicBool::new(false)),
            _help_sub: None,
            update: None,
            install_cb: None,
            update_agreed: Arc::new(AtomicBool::new(false)),
            update_stepped_aside: false,
            ask_by_key: false,
            crumbed_front: None,
            spawn_marks: HashMap::new(),
            s7: surfaces7::Surf7::new(),
            panels,
            convo,
            web,
            downloads: crate::download_ask::DownloadQueue::default(),
            app_asks: crate::app_ask::AppAsks::default(),
            app_launcher: Rc::new(crate::app_ask::RecordingLauncher::default()),
            plugin_ask: None,
            plugin_ask_seq: 0,
            panel_focus_seen: false,
            panel_front_seen: None,
            ctl_events: None,
            ctl_info: None,
            mods: None,
            staged: None,
        };
        let rc = Rc::new(RefCell::new(ctl));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        Self::push_panel_limits(app);
        Self::bind_callbacks(app, &rc);
        Self::make_help(app, &rc);
        rc.borrow_mut().refresh();
        rc
    }

    /// Every key a Settings row stores that the look, the help panel and dictation do not listen to
    /// themselves (the tab position, the sidebar, the keys, the taskbar count, the shell, the folder
    /// for new tabs...): a change posts one `PrefsChanged` per turn, and the view refreshes at once.
    /// Before this the view read them only on its next refresh, so the tab position waited for
    /// Settings to close (owner bug 2026-10-21). Prism Terminal applies every row at once.
    fn listen_to_rows(
        prefs: &PrefStore,
        post: &Post,
        dirty: &Arc<AtomicBool>,
    ) -> Vec<Subscription> {
        let owned: HashSet<&str> = flare_settings::look::LOOK_KEYS
            .iter()
            .chain(flare_settings::agent_prefs::AGENT_LOOK_KEYS.iter())
            .chain(flare_settings::help_prefs::HELP_KEYS.iter())
            .chain(flare_dictation::prefs::KEYS.iter())
            .copied()
            .collect();
        let keys: std::collections::BTreeSet<&str> = flare_settings::rows::all_rows()
            .flat_map(|r| r.keys.iter().copied())
            .filter(|k| !owned.contains(k))
            .collect();
        keys.into_iter()
            .map(|k| {
                let post = Arc::clone(post);
                let dirty = Arc::clone(dirty);
                prefs.on_change(k, move || {
                    if !dirty.swap(true, Ordering::SeqCst) {
                        post(UiMsg::PrefsChanged);
                    }
                })
            })
            .collect()
    }

    /// Makes the update controller over this window (M9 Task 30). The engine is built by the glue
    /// (Task 32) with [`crate::update_ctl::PostEvents`] over this controller's `post`.
    pub fn attach_update(app: &AppWindow, rc: &Rc<RefCell<AppCtl>>, parts: UpdateParts) {
        let (post, agreed) = {
            let c = rc.borrow();
            (Arc::clone(&c.deps.post), Arc::clone(&c.update_agreed))
        };
        let ask_install: AskInstall = {
            let me = Rc::downgrade(rc);
            Rc::new(move |cb| match me.upgrade() {
                Some(rc) => match rc.try_borrow_mut() {
                    Ok(mut c) => c.ask_install(cb),
                    Err(_) => cb(false),
                },
                None => cb(false),
            })
        };
        let quit: Rc<dyn Fn()> = {
            let me = Rc::downgrade(rc);
            Rc::new(move || {
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.quit_for_update();
                }
            })
        };
        let ctl = UpdateCtl::new(
            app,
            UpdateDeps {
                engine: parts.engine,
                post,
                current_version: parts.current_version,
                ask_install,
                agree: Arc::new(move || agreed.store(true, Ordering::SeqCst)),
                spawn: parts.spawn,
                e2e_dir: parts.e2e_dir,
                quit,
            },
        );
        rc.borrow_mut().update = Some(ctl);
    }

    /// The update window is up (the keys and the chords belong to it).
    fn update_up(&self) -> bool {
        self.update
            .as_ref()
            .is_some_and(|u| u.try_borrow().is_ok_and(|u| u.window_up()))
    }

    /// The host's close question for an update install (PT App.tsx `onInstall`): with no agent at
    /// work or present the install may start at once; else the close question is raised in its
    /// install wording and the answer reaches `cb`.
    fn ask_install(&mut self, cb: Box<dyn FnOnce(bool)>) {
        let busy = self.busy_agents();
        let ask = match window_ask(&busy) {
            Some(a) => Some(a),
            None => {
                let labels = self.model.labels();
                self.model
                    .state
                    .tabs
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.kind == TabKind::Shell && self.agent_present(&t.id))
                    .map(|(i, t)| Ask {
                        target: crate::startup::AskTarget::Window,
                        label: labels.get(i).cloned().unwrap_or_default(),
                        agent: self.agent_named(&t.id),
                        for_ms: None,
                        others: 0,
                        install: false,
                    })
            }
        };
        let Some(mut ask) = ask else {
            cb(true);
            return;
        };
        ask.install = true;
        self.drop_install_ask();
        self.install_cb = Some(cb);
        (self.deps.raise)();
        self.ask = Some(ask);
        self.show_ask();
    }

    /// A different question replaces a pending install question: the install is not going ahead.
    fn drop_install_ask(&mut self) {
        if let Some(cb) = self.install_cb.take() {
            cb(false);
        }
    }

    /// The update is installing and the installer asked the app to go: the end of life, then exit.
    fn quit_for_update(&mut self) {
        self.finish();
        (self.deps.quit)();
    }

    /// The Command help controller over this window. It opens on the language of the shell in
    /// front (read from a cell this controller fills just before a toggle), copies through the
    /// clipboard port of the window, and hears its three settings through a message.
    fn make_help(app: &AppWindow, rc: &Rc<RefCell<AppCtl>>) {
        let (prefs, copy, front, still, post, dirty) = {
            let c = rc.borrow();
            let anim = Rc::clone(&c.animations);
            (
                c.prefs.clone(),
                Rc::clone(&c.deps.clipboard),
                Rc::clone(&c.help_front),
                Rc::new(move || !anim()) as Rc<dyn Fn() -> bool>,
                Arc::clone(&c.deps.post),
                Arc::clone(&c.help_dirty),
            )
        };
        let sub = flare_settings::help_prefs::on_help_change(&prefs, move || {
            if !dirty.swap(true, Ordering::SeqCst) {
                post(UiMsg::Help);
            }
        });
        let help = HelpCtl::new(
            app,
            prefs,
            copy,
            Rc::new(move || front.borrow().clone()),
            still,
        );
        let mut c = rc.borrow_mut();
        c.help = Some(help);
        c._help_sub = Some(sub);
    }

    /// F1, the title button and the terminal menu row.
    fn help_toggle(&mut self) {
        // A question is up: the popup waits for the answer.
        if self.ask.is_some() {
            return;
        }
        let Some(h) = self.help.clone() else { return };
        *self.help_front.borrow_mut() = self.front_handle().and_then(|t| t.shell_id());
        if let Ok(mut h) = h.try_borrow_mut() {
            h.toggle();
        }
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
        // The plugins' sections and actions (plugins phase 4).
        bind!(app, rc, on_plugins_clicked, |c| c.plugins_clicked());
        bind!(app, rc, on_plugins_dismissed, |c| c.plugins_dismissed());
        bind!(app, rc, on_plugin_top_clicked, |c, i: i32| c
            .plugin_top_clicked(i));
        bind!(app, rc, on_bar_chip_clicked, |c, i: i32| c
            .bar_chip_clicked(i));
        bind!(
            app,
            rc,
            on_tab_hover_action,
            |c, id: SharedString, i: i32| c.tab_hover_action(&id, i)
        );
        bind!(app, rc, on_plugin_fold, |c, sid: SharedString| c
            .plugin_fold(&sid));
        bind!(
            app,
            rc,
            on_plugin_header_menu,
            |c, sid: SharedString, x: f32, y: f32| { c.plugin_header_menu(&sid, x, y) }
        );
        bind!(
            app,
            rc,
            on_plugin_item,
            |c, sid: SharedString, id: SharedString, on_box: bool| {
                c.plugin_item(&sid, &id, on_box)
            }
        );
        bind!(app, rc, on_plus_menu, |c, x: f32, y: f32| c
            .open_plus_menu(x, y));
        bind!(
            app,
            rc,
            on_folder_menu,
            |c, right: f32, top: f32, foot: f32| c.open_folder_menu(right, top, foot)
        );
        bind!(app, rc, on_more_clicked, |c, x: f32, y: f32| c
            .open_overflow(false, x, y));
        bind!(app, rc, on_settings_clicked, |c| c.toggle_settings());
        bind!(app, rc, on_help_clicked, |c| c.help_toggle());
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
        bind!(
            app,
            rc,
            on_staged_chip_clicked,
            |c, r: f32, t: f32, f: f32| c.staged_chip_clicked(r, t, f)
        );
        bind!(app, rc, on_staged_toast_clicked, |c| c
            .staged_toast_clicked());
        bind!(app, rc, on_menu_trailing, |c, i: i32| c.menu_trailing(i));
        bind!(app, rc, on_menu_dismissed, |c| c.close_menu());
        bind!(app, rc, on_menu_outside_right, |c, x: f32, y: f32| c
            .outside_right_click(x, y));
        bind!(app, rc, on_overflow_outside_right, |c, x: f32, y: f32| c
            .outside_right_click(x, y));
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
        bind!(app, rc, on_pask_fill, |c, choice: i32| c
            .answer_fill(choice));
        bind!(
            app,
            rc,
            on_pask_secret,
            |c, ok: bool, text: SharedString| c.answer_secret(ok, text)
        );
        bind!(
            app,
            rc,
            on_tip_request,
            |c, t: SharedString, x: f32, y: f32, w: f32, h: f32| c.tip_request(&t, x, y, w, h)
        );
        bind!(
            app,
            rc,
            on_panel_ask_tip,
            |c, t: SharedString, x: f32, y: f32, w: f32, h: f32| {
                c.tip_request_toward(&t, x, y, w, h, Some(TipSide::Left));
            }
        );
        // The Settings view's tooltips go the same way as every other surface's.
        {
            let weak = Rc::downgrade(rc);
            // untimed: a tooltip request only records the hover and returns at once
            app.global::<SettingsBridge>().on_tip(
                move |t: SharedString, x: f32, y: f32, w: f32, h: f32| {
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.tip_request(&t, x, y, w, h);
                    }
                },
            );
        }
        // The e2e probe keeps what a model row drew; a release build never binds it (audit L42).
        if crate::e2e::global().on() {
            // untimed: a model row's look is noted in a map and returns at once
            app.global::<SettingsBridge>()
                .on_row_look(crate::e2e_probes::dictation::note_look);
        }
        {
            let weak = Rc::downgrade(rc);
            // untimed: a tooltip request only records the hover and returns at once
            app.global::<HelpBridge>().on_tip(
                move |t: SharedString, x: f32, y: f32, w: f32, h: f32| {
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.tip_request(&t, x, y, w, h);
                    }
                },
            );
        }
        bind!(app, rc, on_bar_double_clicked, |c| c.toggle_maximise());
        bind!(app, rc, on_title_pressed, |c| c.title_pressed());
        bind!(
            app,
            rc,
            on_grid_pointer,
            |c, kind: i32, button: i32, x: f32, y: f32, ctrl: bool, shift: bool, alt: bool| {
                c.grid_pointer(PointerIn {
                    kind: kind.clamp(0, 3) as u8,
                    button: button.clamp(0, 3) as u8,
                    x_css: f64::from(x),
                    y_css: f64::from(y),
                    ctrl,
                    shift,
                    alt,
                })
            }
        );
        bind!(app, rc, on_grid_pointer_left, |c| c.grid_pointer_left());
        bind!(app, rc, on_find_edited, |c, q: SharedString| c
            .term_find_edited(&q));
        bind!(app, rc, on_find_next, |c| c.term_find_step(true));
        bind!(app, rc, on_find_prev, |c| c.term_find_step(false));
        bind!(app, rc, on_find_close, |c| c.term_find_close());
        bind!(app, rc, on_panel_close, |c| c.close_front_panel());
        bind!(app, rc, on_panel_escape, |c| c.panel_escape());
        bind!(app, rc, on_panel_convo_select, |c, path: SharedString| c
            .convo_select(path.as_str()));
        bind!(app, rc, on_panel_convo_open, |c, path: SharedString| c
            .convo_open(path.as_str()));
        bind!(app, rc, on_panel_wrap_toggled, |c| c.panel_wrap_toggled());
        bind!(app, rc, on_panel_md_view_picked, |c, i: i32| c
            .panel_md_view_picked(i));
        bind!(app, rc, on_panel_ask_agent, |c| c.panel_ask_agent());
        bind!(app, rc, on_panel_wrap_cols_changed, |c| c
            .panel_wrap_cols_changed());
        bind!(app, rc, on_preview_chip_clicked, |c| c
            .preview_chip_clicked());
        bind!(app, rc, on_panel_pressed, |c| c.panel_focus_seen = true);
        bind!(app, rc, on_panel_splitter_released, |c, share: f32| c
            .panel_splitter_released(share));
        bind!(app, rc, on_panel_web_back, |c| c
            .web_nav(flare_web::Nav::Back));
        bind!(app, rc, on_panel_web_forward, |c| c
            .web_nav(flare_web::Nav::Forward));
        bind!(app, rc, on_panel_web_reload, |c| c
            .web_nav(flare_web::Nav::Reload));
        bind!(app, rc, on_panel_web_stop, |c| c
            .web_nav(flare_web::Nav::Stop));
        bind!(app, rc, on_panel_web_submit, |c, text: SharedString| c
            .web_submit(&text));
        bind!(app, rc, on_panel_web_download, |c| c.web_download());
        bind!(app, rc, on_panel_web_popup_close, |c| c.web_popup_close());
        bind!(app, rc, on_panel_web_moved, |c| c.web_moved());
        bind!(app, rc, on_overlay_changed, |c| c.web_moved());
    }

    /// The terminal controller over this window: its closures reach the window only through a weak
    /// handle, so a message or a redraw after the window is gone does nothing.
    fn make_term(app: &AppWindow, deps: &Deps, agent: Rc<dyn AgentView>) -> TermUi {
        let weak = app.as_weak();
        let dirty = Rc::clone(&deps.term.mark_grid_dirty);
        let redraw: Rc<dyn Fn()> = {
            let weak = weak.clone();
            Rc::new(move || {
                dirty();
                if let Some(app) = weak.upgrade() {
                    app.window().request_redraw();
                }
            })
        };
        let set_cursor: Rc<dyn Fn(CursorKind)> = {
            let weak = weak.clone();
            Rc::new(move |k| {
                if let Some(app) = weak.upgrade() {
                    app.set_grid_cursor(cursor_of(k));
                }
            })
        };
        let set_thumb: SetThumb = {
            let weak = weak.clone();
            Rc::new(move |v| {
                if let Some(app) = weak.upgrade() {
                    show_thumb(&app, v);
                }
            })
        };
        let post = Arc::clone(&deps.post);
        TermUi::new(TermDeps {
            clipboard: Arc::clone(&deps.term.clip_port),
            openers: Arc::clone(&deps.term.openers),
            open_url: Rc::clone(&deps.open_url),
            post: Arc::new(move |m| post(UiMsg::Term(TermMsgBox(m)))),
            clock_ms: Rc::clone(&deps.clock),
            redraw,
            set_cursor,
            set_thumb,
            agent,
            home: deps.home.clone(),
            dbl: deps.term.dbl,
            stat: Arc::clone(&deps.term.stat),
            right_click_reports: {
                let prefs = deps.prefs.clone();
                Rc::new(move || flare_settings::term_prefs::right_click_reports(&prefs))
            },
        })
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

    pub(crate) fn front_id(&self) -> Option<String> {
        self.model.state.active_id.clone()
    }

    fn front_handle(&self) -> Option<Arc<TermHandle>> {
        let t = self.model.active_shell()?;
        self.deps.host.handle(&t.id)
    }

    /// Whether the front tab's grid shows a non-empty row (the start-up trace's prompt test).
    fn front_shows_prompt(&self) -> bool {
        self.front_handle()
            .is_some_and(|h| h.screen_tail(1).iter().any(|r| !r.trim().is_empty()))
    }

    /// What the grid source reads each frame: the selection, the find matches, the links.
    pub fn decor_inputs(&self) -> Rc<RefCell<crate::term_ui::DecorInputs>> {
        self.term.inputs()
    }

    fn focus_terminal(&mut self) {
        if let Some(app) = self.app() {
            // The page may hold the Windows keyboard focus: take it back, or the terminal types nothing.
            let hwnd = crate::window::hwnd_of(&app);
            self.web.release_focus(hwnd);
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
        self.staged_note_busy();
        let now = self.now();
        let front = self.model.state.active_id.clone();
        if front != self.crumbed_front {
            if let Some(id) = &front {
                crate::diag_ctl::crumb_often("tab-switch", &[("id", crate::diag_ctl::txt(id))]);
            }
            self.crumbed_front = front;
        }
        let scale = f64::from(app.get_scale());
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let strip_phys = app.get_strip_phys_width();
        if let Some(u) = self.update.clone()
            && let Ok(mut u) = u.try_borrow_mut()
        {
            let size = app.window().size();
            let sc = self.scale as f32;
            u.on_scale_or_size(size.width as f32 / sc, size.height as f32 / sc, sc);
        }
        self.sync_hub();
        if self.prune_restoring(now) {
            self.sync_model_agents();
        }
        self.sync_looking();
        self.sync_marks(&app);
        let none: HashMap<String, f32> = HashMap::new();
        let base = {
            let ind = self.hub.indicator();
            let mut kinds = HashMap::new();
            let mut failed_kinds = HashMap::new();
            // The agents the hub knows, plus the tabs coming back to one: a placeholder with a
            // saved agent and a restored tab whose agent has not painted yet wear the agent's icon
            // and the loading ring from the first frame (they are not new work: no mark).
            let mut agents: HashSet<String> = ind.agent_ids().clone();
            let mut loading = HashSet::new();
            for t in &self.model.state.tabs {
                if let Some(k) = self.hub.agent_kind(&t.id) {
                    kinds.insert(t.id.clone(), k);
                } else if let Some((k, _)) = self.restoring.get(&t.id) {
                    kinds.insert(t.id.clone(), detected(Some(*k)));
                    agents.insert(t.id.clone());
                    if self.deps.host.handle(&t.id).is_some_and(|h| h.resuming()) {
                        loading.insert(t.id.clone());
                    }
                } else if t.pending.is_some()
                    && let Some(k) = self.model.agent_kind(&t.id)
                {
                    kinds.insert(t.id.clone(), detected(Some(k)));
                    agents.insert(t.id.clone());
                }
                if let Some(f) = self.hub.failed_kind(&t.id) {
                    failed_kinds.insert(t.id.clone(), f.to_string());
                }
            }
            let agent = AgentRows {
                sets: TabSets {
                    agents: &agents,
                    working: ind.working_ids(),
                    question: ind.question_ids(),
                    failed: ind.failed_ids(),
                    done: ind.done_ids(),
                },
                sw: switches_of(&self.prefs),
                marks: &self.marks,
                kinds,
                failed_kinds,
                loading,
            };
            let needs = needs_of(self.model.state.tabs.iter().map(|t| t.id.as_str()), &agent);
            build_view(&ViewInput {
                model: &self.model,
                home: &self.deps.home,
                query: &self.query,
                strip_phys,
                scale: self.scale,
                needs: &needs,
                offsets: &none,
                carried: None,
                agent: &agent,
            })
        };
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
        match self.mods.as_ref().map(|m| Arc::clone(m.hub())) {
            Some(hub) => crate::tabs_ui::decorate_with(
                &mut view.rows,
                &self.decor,
                self.scale,
                &|id, icon| hub.icon_of(id, icon),
                &|id| hub.name_of(id),
            ),
            None => crate::tabs_ui::decorate(&mut view.rows, &self.decor, self.scale),
        }
        self.plugin_hover_rows(&mut view.rows);
        crate::tabs_ui::wear_motions(&mut view.rows, &self.motions);

        sync_rows(&self.rows_model, &view.rows);
        self.push_plugins(&app);
        if self.last_geom.as_ref() != Some(&view.geom) {
            let dots: Vec<slint::Color> = view.geom.dots.iter().map(|d| d.colour).collect();
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
        app.set_help_key(self.key_label_of(ChordId::Help).into());
        app.set_find_tab_key(self.key_label_of(ChordId::FindTab).into());
        app.set_new_tab_title(
            format!("New tab ({new_key}). Right-click for recent folders").into(),
        );
        // The sidebar's button has an arrow for the folders instead of a right-click (2026-10-27).
        app.set_new_tab_side_title(format!("New tab ({new_key})").into());
        app.set_version_text(format!("Version {}", crate::app_facts::version()).into());

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
        self.aim_slide(&app, &sv);
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
        self.sync_settings();

        // The grid draws whichever shell is in front; Settings, a placeholder and no tab draw none.
        let want = self
            .model
            .active_shell()
            .and_then(|t| self.deps.host.handle(&t.id).map(|h| (t.id.clone(), h)));
        let want_id = want.as_ref().map(|w| w.0.clone());
        if want_id != self.source_id {
            self.term.tab_changed(want_id.as_deref());
            if matches!(
                self.menu.as_ref().map(|m| &m.kind),
                Some(MenuKind::Term { .. })
            ) {
                self.close_menu_quietly();
            }
            self.source_id = want_id;
            (self.deps.set_grid_source)(want.map(|w| w.1));
            // The tab in front may wear its own zoom.
            self.apply_font();
        }

        // The panel of the tab in front, and the previews switch the terminal reads.
        self.sync_panels(&app);
        self.notify_update();
        self.term
            .set_previews(flare_settings::term_prefs::open_previews(&self.prefs));
        let thumbs_on = flare_settings::term_prefs::hover_thumbs(&self.prefs);
        self.term.set_thumbs(thumbs_on);
        if !thumbs_on && !app.get_thumb_caption().is_empty() {
            app.set_thumb_caption("".into());
            app.set_thumb_picture(slint::Image::default());
        }

        // Rule 37: nothing is written before the restore answered. Nor after the end of life: `finish` wrote
        // the tabs as they stood, then killed the shells, and a refresh after that sees every agent gone and
        // would write a tabs.json with no agent to resume (2026-10-25: three restarts came back as plain shells).
        if !self.ended
            && let Some(s) = self.model.saved_if_restored()
            && self.last_saved.as_ref() != Some(&s)
        {
            self.deps.tabs_store.save(&s);
            self.last_saved = Some(s);
        }
        if !self.ended && self.model.restore_done() {
            let tabs = self.shell_tabs();
            self.panels.persist(&tabs);
        }

        self.push_term();
        self.update_skeleton(now);
        self.update_clock();
        self.update_badge();
        // What changed on the tabs since the last refresh goes to the plugins that listen (one round a refresh).
        self.plugin_events();
        app.window().request_redraw();
        self.kick();
    }

    // ---- the agent hub and the mark clock -----------------------------------------------------

    /// Starts the process poll (production; a test world has none).
    pub fn start_agent_poll(&mut self) {
        self.hub
            .start_poll(crate::agent_hub::PollSource::Real(self.deps.host.manager()));
    }

    /// The overlay icon on the taskbar button, made once the window has a handle.
    pub fn set_taskbar_badge(&mut self, badge: TaskbarBadge) {
        self.badge = Some(badge);
        self.update_badge();
    }

    /// The count of tabs that want a look, on the taskbar button; cleared when none, or when the
    /// setting is off. A repeat of the same count at the same size draws nothing.
    fn update_badge(&mut self) {
        let n = self.hub.attention(&switches_of(&self.prefs)).count;
        let on = taskbar_badge_on(&self.prefs);
        let scale = self.scale;
        let Some(b) = self.badge.as_mut() else { return };
        let ok = if !on || n == 0 {
            b.clear()
        } else {
            b.set(n, scale)
        };
        if self.badge_seen != Some((n, on, ok)) {
            self.badge_seen = Some((n, on, ok));
            let mut fields = vec![
                ("count", serde_json::json!(n)),
                ("on", serde_json::json!(on)),
                ("ok", serde_json::json!(ok)),
            ];
            if !ok {
                fields.push((
                    "why",
                    crate::diag_ctl::txt(&flare_win::taskbar::last_shell_error()),
                ));
            }
            crate::diag_ctl::crumb("taskbar-badge", &fields);
        }
    }

    /// The agent a tab hosts, as far as anything knows: the indicator, or the saved agent of a tab
    /// coming back.
    fn agent_present(&self, id: &str) -> bool {
        self.hub.agent_here(id) || self.model.has_agent(id)
    }

    fn agent_named(&self, id: &str) -> DetectedAgent {
        self.hub
            .agent_kind(id)
            .filter(|_| self.hub.agent_here(id))
            .unwrap_or_else(|| detected(self.model.agent_kind(id)))
    }

    /// How long the agent in the tab has been working, if it is.
    fn working_for_ms(&self, id: &str) -> Option<u64> {
        if self.hub.working(id) {
            return self.hub.working_for(id);
        }
        self.working
            .get(id)
            .map(|since| self.now().saturating_sub(*since))
    }

    /// `tabs.json` records an agent only for a present Claude or Codex (never `other`), and for a
    /// tab coming back to one until the hub has seen it.
    fn sync_model_agents(&mut self) {
        let ids: Vec<String> = self
            .model
            .state
            .tabs
            .iter()
            .filter(|t| t.kind == TabKind::Shell && t.pending.is_none())
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            let want = self
                .hub
                .saved_agent(&id)
                .or_else(|| self.restoring.get(&id).map(|(k, _)| *k));
            if self.model.agent_kind(&id) != want {
                self.model.set_agent(&id, want);
            }
        }
        self.close_left_agent_panels();
    }

    /// The agent in `tab` opened its panel: the panel goes when that agent leaves.
    fn mark_agent_panel(&mut self, tab: &str) {
        self.panels.mark_agent(tab);
        if self.hub.agent_here(tab) {
            self.agent_panel_seen.insert(tab.to_string());
        }
    }

    /// A panel its agent opened closes once the hub, having seen that agent in the tab, no longer does. A tab
    /// whose agent has not been seen yet (a resume still starting) keeps its panel. The caller refreshes.
    fn close_left_agent_panels(&mut self) {
        let tabs = self.panels.agent_tabs();
        self.agent_panel_seen.retain(|t| tabs.contains(t));
        for tab in tabs {
            if self.hub.agent_here(&tab) {
                self.agent_panel_seen.insert(tab);
            } else if self.agent_panel_seen.remove(&tab) && self.panels.close_for_agent(&tab) {
                self.panel_changed(&tab);
            }
        }
    }

    /// A restored tab stops wearing its saved agent once the hub has seen the agent, or once the
    /// resume is over and the grace has run out. True when one was dropped.
    fn prune_restoring(&mut self, now: u64) -> bool {
        let gone: Vec<String> = self
            .restoring
            .iter()
            .filter(|(id, (_, at))| {
                let tab = self.model.state.tabs.iter().any(|t| &t.id == *id);
                let resuming = self.deps.host.handle(id).is_some_and(|h| h.resuming());
                !tab || self.hub.agent_here(id)
                    || (!resuming && now.saturating_sub(*at) >= RESTORE_GRACE_MS)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let any = !gone.is_empty();
        for id in gone {
            self.restoring.remove(&id);
        }
        any
    }

    /// What the hub is told is being looked at: the shell in front, and whether the window has the
    /// keyboard focus. Told on a change only.
    fn sync_looking(&mut self) {
        let front = self.model.active_shell().map(|t| t.id.clone());
        let now = (front, self.focused);
        if self.looked.as_ref() == Some(&now) {
            return;
        }
        self.hub.set_looking(now.0.as_deref(), now.1);
        self.looked = Some(now);
    }

    /// Where a bell's flash goes (the window sets it once it has a handle).
    pub fn set_flash(&mut self, flash: Rc<dyn Fn()>) {
        self.flash = flash;
    }

    /// The window gained or lost the keyboard focus. Coming back is not a look for a mark held while
    /// away (2026-10-21); it still takes down a background tab's mark when that tab is in front.
    pub fn on_focus(&mut self, focused: bool) {
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        // Coming back draws the taskbar badge anew: Explorer may have restarted since, which takes
        // every overlay away, or the button did not exist yet when the badge was first set.
        if focused && let Some(b) = self.badge.as_mut() {
            b.refresh();
            self.update_badge();
        }
        self.sync_looking();
        if self.hub.take_changed() {
            self.sync_model_agents();
            self.refresh();
        }
        // Looking away from a question (or back at it) is the notifier's business.
        self.notify_update();
    }

    /// The resume skeleton: shown while the tab in front resumes (or is a placeholder for an
    /// agent), faded for 180 ms once the agent has painted, then gone.
    fn update_skeleton(&mut self, now: u64) {
        let front_resuming = self.front_handle().is_some_and(|h| h.resuming());
        let front_placeholder = self
            .model
            .state
            .active_id
            .as_deref()
            .and_then(|a| self.model.state.tabs.iter().find(|t| t.id == a))
            .is_some_and(|t| t.pending == Some(Pending::Agent));
        let want = front_resuming || front_placeholder;
        let before = self.skel;
        if front_resuming {
            self.skel_resumed = true;
        }
        self.skel = match (self.skel, want) {
            (_, true) => Skel::Shown,
            // The agent painted: the page fades. A placeholder whose tab turned out to be a plain
            // shell (no session to resume) goes at once.
            (Skel::Shown, false) if self.skel_resumed => Skel::Leaving(now + SKELETON_FADE_MS),
            (Skel::Shown, false) => Skel::Gone,
            (Skel::Leaving(t), false) if now >= t => Skel::Gone,
            (s, false) => s,
        };
        if self.skel == Skel::Gone {
            self.skel_resumed = false;
        }
        if self.skel == before {
            return;
        }
        self.skeleton.set(self.skel != Skel::Gone);
        if let Some(app) = self.app() {
            app.set_skeleton_shown(self.skel != Skel::Gone);
            app.set_skeleton_leaving(matches!(self.skel, Skel::Leaving(_)));
        }
    }

    /// When the skeleton's fade ends, if one is running.
    fn skeleton_due(&self) -> Option<u64> {
        match self.skel {
            Skel::Leaving(t) => Some(t),
            _ => None,
        }
    }

    /// The hub sees the host's handles as they are now, and forgets a tab that has gone.
    fn sync_hub(&mut self) {
        let live: HashMap<String, Arc<TermHandle>> = self
            .model
            .state
            .tabs
            .iter()
            .filter_map(|t| self.deps.host.handle(&t.id).map(|h| (t.id.clone(), h)))
            .collect();
        let gone: Vec<String> = self
            .hub_known
            .iter()
            .filter(|id| !live.contains_key(*id))
            .cloned()
            .collect();
        // A tab that is new wears the colours the look already wears, so its colour queries and
        // theme reports are right from its first byte.
        let fresh: Vec<Arc<TermHandle>> = live
            .iter()
            .filter(|(id, _)| !self.hub_known.contains(*id))
            .map(|(_, h)| Arc::clone(h))
            .collect();
        if !fresh.is_empty() {
            crate::look_ctl::LookCtl::with_current(|l| {
                let term = l.current_look().term;
                for h in &fresh {
                    h.set_colours(&term.foreground, &term.background);
                }
            });
        }
        *self.handles.borrow_mut() = live;
        for id in gone {
            self.hub.on_closed(&id);
            // Whatever was put on the tab goes with it.
            self.decor.remove_tab(&id);
            self.plugin_tab_gone(&id);
            // A recording that was to be pasted into this tab learns it has closed.
            crate::dictation_ctl::tab_gone(&id);
        }
        self.hub_known = self.handles.borrow().keys().cloned().collect();
    }

    /// The rainbow switch moves the stops with it; the other changes arrive as `LookChanged`.
    fn sync_marks(&mut self, app: &AppWindow) {
        let rainbow = agent_rainbow(&self.prefs);
        if rainbow != self.marks_rainbow {
            self.marks_rainbow = rainbow;
            self.marks = compute_marks(&self.prefs);
            Self::push_rainbow(app, &self.marks);
        }
    }

    fn push_rainbow(app: &AppWindow, marks: &UiMarks) {
        app.global::<MarkMotion>()
            .set_rainbow(ModelRc::new(VecModel::from(marks.rainbow.clone())));
    }

    /// The theme, the indicator, a pick or a switch changed: the marks wear the new colours, and the
    /// grid wears the terminal font and size in force (a no-op when they did not change).
    pub fn on_look_changed(&mut self) {
        self.marks = compute_marks(&self.prefs);
        self.marks_rainbow = agent_rainbow(&self.prefs);
        if let Some(app) = self.app() {
            Self::push_rainbow(&app, &self.marks);
        }
        self.apply_font();
        self.refresh();
    }

    /// A setting the tab view reads changed: the view refreshes now (the coalescing flag is cleared
    /// first, so a change made by the refresh itself is heard again).
    pub fn on_prefs_changed(&mut self) {
        self.prefs_dirty.store(false, Ordering::SeqCst);
        self.dev_settings_changed();
        self.refresh();
    }

    /// Windows' animation-effects switch (tests inject one).
    pub fn set_animations_source(&mut self, f: Rc<dyn Fn() -> bool>) {
        self.web.set_motion(Rc::clone(&f));
        self.animations = f;
        self.update_clock();
    }

    /// The resume skeleton shows (or no longer shows) over the front tab.
    pub fn set_skeleton_shown(&mut self, shown: bool) {
        self.skeleton.set(shown);
        self.update_clock();
    }

    /// The window was minimised or restored: nothing moves while it is down, and the marks go on
    /// from where they stood.
    pub fn on_minimised(&mut self, minimised: bool) {
        if minimised == self.minimised {
            return;
        }
        self.minimised = minimised;
        let now = self.now();
        self.clock.pause.borrow_mut().set_minimized(minimised, now);
        self.update_clock();
        self.sync_chip();
    }

    /// Whether a minimised window holds the marks (what `marks.state` reports as `paused`).
    pub fn marks_paused(&self) -> bool {
        self.minimised
    }

    /// Whether the mark clock's timer is running (it must not while no mark moves).
    pub fn mark_clock_running(&self) -> bool {
        self.clock.timer.running()
    }

    /// Decides after every refresh whether the clock runs: a mark that moves, a window that shows
    /// and Windows' animation effects on. Otherwise the timer is stopped and the marks stand still.
    fn update_clock(&mut self) {
        let Some(app) = self.app() else { return };
        let states: Vec<MarkState> = self
            .view
            .rows
            .iter()
            .filter_map(|r| mark_state_of(r.mark_state))
            .collect();
        let loading = self.view.rows.iter().any(|r| r.loading) || self.skeleton.get();
        let sw = switches_of(&self.prefs);
        let need = needs_clock(&states, sw.indicator, sw.rainbow, loading) && (self.animations)();
        if !need {
            self.clock.timer.stop();
            self.clock.start = None;
            push_frame(&app, STILL, &ICON_STILL, 1.0);
            return;
        }
        if self.minimised {
            // Frozen where they stood.
            self.clock.timer.stop();
            return;
        }
        let now = self.now();
        let start = *self
            .clock
            .start
            .get_or_insert_with(|| self.clock.pause.borrow().clock(now));
        if !self.clock.timer.running() {
            let weak = app.as_weak();
            let pause = Rc::clone(&self.clock.pause);
            let clock = Rc::clone(&self.deps.clock);
            let skeleton = Rc::clone(&self.skeleton);
            self.clock.timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(MARK_FRAME_MS),
                move || {
                    let Some(app) = weak.upgrade() else { return };
                    let now = clock();
                    if app.window().is_minimized() {
                        pause.borrow_mut().set_minimized(true, now);
                        return;
                    }
                    crate::diag_ctl::timed("mark_frame", || {
                        Self::mark_frame(&app, &pause.borrow(), now, start, skeleton.get());
                    });
                },
            );
        }
        Self::mark_frame(
            &app,
            &self.clock.pause.borrow(),
            now,
            start,
            self.skeleton.get(),
        );
    }

    /// One frame of every moving mark at `now`.
    fn mark_frame(app: &AppWindow, pause: &MarksPause, now: u64, start: u64, skeleton: bool) {
        let ms = pause.clock(now).saturating_sub(start);
        let pulse = if skeleton { pulse_at(ms) } else { 1.0 };
        push_frame(app, mark_phase(ms), &icon_phase(ms), pulse);
    }

    /// Runs the clock's body once at the controller's clock (the timer's own tick, for tests).
    #[cfg(test)]
    pub(crate) fn mark_frame_now(&self) {
        let Some(app) = self.app() else { return };
        if let Some(start) = self.clock.start {
            Self::mark_frame(
                &app,
                &self.clock.pause.borrow(),
                self.now(),
                start,
                self.skeleton.get(),
            );
        }
    }

    /// The hub, for the tests and the headless scenes (a scene answers the process poll itself).
    pub(crate) fn hub_mut(&mut self) -> &mut AgentHub {
        &mut self.hub
    }

    /// The hub, read only: what the test hook's agent probe asks.
    pub(crate) fn hub(&self) -> &AgentHub {
        &self.hub
    }

    /// What dictation needs at install: the settings store and the session manager (M8 Task 22).
    pub(crate) fn dictation_inputs(&self) -> crate::dictation_ctl::DictInputs {
        crate::dictation_ctl::DictInputs {
            prefs: self.prefs.clone(),
            mgr: self.deps.host.manager(),
        }
    }

    /// The shell tab in front with a live session: where dictation types. `None` on the Settings page,
    /// the Explorer, and a shell that has not started.
    pub(crate) fn dictation_front(&self) -> Option<String> {
        let t = self.model.active_shell()?;
        self.deps.host.handle(&t.id).map(|_| t.id.clone())
    }

    /// The clipboard dictation keeps unsent words on.
    pub(crate) fn dictation_clip(&self) -> Arc<dyn ClipboardPort> {
        Arc::clone(&self.deps.term.clip_port)
    }

    /// The tabs as the dictation send rules see them: the hub's, each named by its folder.
    pub(crate) fn dictation_tabs(&self) -> Vec<flare_dictation::route::SendTab> {
        self.hub.send_tabs()
    }

    /// The live terminal of a tab (dictation pastes through it; window flow tests send keys through it).
    pub(crate) fn handle_of(&self, id: &str) -> Option<Arc<TermHandle>> {
        self.deps.host.handle(id)
    }

    /// The close question as the dialog shows it: title, body text and primary button.
    #[cfg(test)]
    pub(crate) fn ask_text(&self) -> Option<(String, String, String)> {
        self.ask.as_ref().map(|a| {
            let v = crate::startup::ask_view(a, "#ffffff");
            (
                v.title.to_string(),
                v.body.to_string(),
                v.primary.to_string(),
            )
        })
    }

    /// Which agent the keyboard path believes is armed in the tab.
    #[cfg(test)]
    pub(crate) fn armed_agent(&self, id: &str) -> Option<AgentKind> {
        self.agent_view.armed(id)
    }

    // ---- the restore ----------------------------------------------------------------------------

    /// A worker thread resolves the saved strip; [`UiMsg::RestoreDone`] answers. Runs ONCE. The folder menu's
    /// starter folders are asked for here too, once; [`UiMsg::StarterFolders`] answers.
    pub fn start_restore(this: &Rc<RefCell<AppCtl>>) {
        let (saved, is_dir, lookup, post, starters) = {
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
                Rc::clone(&c.deps.starter_folders),
            )
        };
        {
            let post = Arc::clone(&post);
            starters(Arc::new(move |list| post(UiMsg::StarterFolders(list))));
        }
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
        self.restore_panels();
        let now = self.now();
        for s in spawns {
            self.deps.host.spawn(&s.id, &s.cwd, s.resume.as_deref());
            self.note_spawn(&s.id, &s.cwd, s.resume.is_some());
            self.hub.on_spawned(&s.id, &s.cwd, s.resume.as_deref());
            // Only an id that passed the check is a resume; the rest is a plain shell.
            let kind = valid_resume(s.resume.as_deref()).map(agent_of_resume);
            self.model.set_agent(&s.id, kind);
            if let Some(k) = kind {
                self.restoring.insert(s.id.clone(), (k, now));
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
    pub(crate) fn open_folder(&mut self, dir: &str) -> String {
        let id = self.model.open_tab(dir);
        self.deps.host.spawn(&id, dir, None);
        self.note_spawn(&id, dir, false);
        self.hub.on_spawned(&id, dir, None);
        self.refresh();
        id
    }

    /// A tab was added and its shell asked for: the `tab-open` crumb, and the clock of `shell-spawn`.
    fn note_spawn(&mut self, id: &str, cwd: &str, resume: bool) {
        crate::startup_trace::mark(crate::startup_trace::Phase::ShellSpawn);
        crate::diag_ctl::crumb(
            "tab-open",
            &[
                ("id", crate::diag_ctl::txt(id)),
                ("cwd", crate::diag_ctl::txt(cwd)),
                ("resume", serde_json::Value::Bool(resume)),
            ],
        );
        self.spawn_marks.insert(
            id.to_owned(),
            (std::time::Instant::now(), cwd.to_owned(), resume),
        );
    }

    /// A tab goes: the `tab-close` crumb, `kind` being the agent in it (`claude`, `codex`, `agy`) or `shell`.
    fn note_close(&mut self, id: &str) {
        let kind = self.model.agent_kind(id).map_or("shell", AgentKind::as_str);
        crate::diag_ctl::crumb(
            "tab-close",
            &[
                ("id", crate::diag_ctl::txt(id)),
                ("kind", crate::diag_ctl::txt(kind)),
            ],
        );
        self.spawn_marks.remove(id);
    }

    /// What a shell reported: `shell-spawn` (how long the start took), `shell-spawn-failed`, `shell-exit`.
    fn shell_crumb(&mut self, ev: &HostEvent) {
        use serde_json::Value;
        let pid_of = |ctl: &Self, id: &str| -> Option<u32> {
            ctl.deps
                .host
                .manager()
                .live_pids()
                .into_iter()
                .find(|(t, _)| t == id)
                .map(|(_, pid)| pid)
        };
        match ev {
            HostEvent::Started { id } => {
                let (ms, cwd, resume) = self.spawn_marks.get(id).map_or_else(
                    || (0, String::new(), false),
                    |(at, cwd, resume)| (at.elapsed().as_millis() as u64, cwd.clone(), *resume),
                );
                let shell = self.deps.host.shell_id_of(id).flatten().unwrap_or_default();
                crate::diag_ctl::crumb(
                    "shell-spawn",
                    &[
                        ("id", crate::diag_ctl::txt(id)),
                        ("pid", Value::from(pid_of(self, id).unwrap_or(0))),
                        ("shell", Value::String(shell)),
                        ("cwd", Value::String(cwd)),
                        ("resume", Value::Bool(resume)),
                        ("ms", Value::from(ms)),
                    ],
                );
            }
            HostEvent::StartFailed { id } => {
                let (cwd, _) = self
                    .spawn_marks
                    .get(id)
                    .map_or((String::new(), false), |(_, c, r)| (c.clone(), *r));
                let shell = self.deps.host.shell_id_of(id).flatten().unwrap_or_default();
                crate::diag_ctl::crumb(
                    "shell-spawn-failed",
                    &[
                        ("id", crate::diag_ctl::txt(id)),
                        ("shell", Value::String(shell)),
                        ("cwd", Value::String(cwd)),
                        ("err", crate::diag_ctl::txt("start failed")),
                    ],
                );
            }
            HostEvent::Exited { id, code } => {
                crate::diag_ctl::crumb(
                    "shell-exit",
                    &[
                        ("id", crate::diag_ctl::txt(id)),
                        ("pid", Value::from(pid_of(self, id).unwrap_or(0))),
                        ("exitCode", code.map_or(Value::Null, Value::from)),
                    ],
                );
            }
            _ => {}
        }
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
        self.deps.host.prewarm(&self.new_tab_dir());
    }

    /// The + button and Ctrl+T: the stored folder, else the user's folder, no question. (There is no ask mode
    /// any more, 2026-10-27: a stored "ask" opens here too; the folder menu's first row asks instead.)
    pub fn new_tab(&mut self) {
        self.close_menu_quietly();
        let dir = self.new_tab_dir();
        self.open_folder(&dir);
        self.deps.host.prewarm(&dir);
        self.focus_terminal();
    }

    /// The folder menu's "Choose a folder…": the folder picker, starting in the new-tab folder; the answer opens a
    /// tab there ([`UiMsg::Picked`]), and the spawn remembers it as a recent. A cancel opens nothing.
    fn choose_folder(&mut self) {
        let default_dir = Some(PathBuf::from(self.new_tab_dir()));
        let post = Arc::clone(&self.deps.post);
        let answer: PickAnswer = Arc::new(move |p| post(UiMsg::Picked(p)));
        (self.deps.pick_folder)(default_dir, answer);
    }

    /// Closing asks first when the tab hosts an agent, else closes at once.
    pub fn request_close(&mut self, id: &str) {
        if !self.model.state.tabs.iter().any(|t| t.id == id) {
            return;
        }
        if flare_agent::close::asks_before_closing_tab(self.agent_present(id)) {
            let label = self
                .model
                .state
                .tabs
                .iter()
                .position(|t| t.id == id)
                .and_then(|i| self.model.labels().get(i).cloned())
                .unwrap_or_default();
            let for_ms = self.working_for_ms(id);
            self.drop_install_ask();
            self.ask = Some(tab_ask(id, &label, self.agent_named(id), for_ms));
            self.show_ask();
            return;
        }
        self.close_now(id);
    }

    pub(crate) fn close_now(&mut self, id: &str) {
        self.note_close(id);
        self.deps.host.kill(id);
        self.working.remove(id);
        self.model.close(id);
        self.term.tab_closed(id);
        self.forget_tab_surfaces(id);
        self.refresh();
    }

    /// Everything the Surfaces keep for a tab that is gone: its panel (and mini-app), its tracker,
    /// its dev chip, its notifier row and its browser. One list for a close and for a shell that exited.
    fn forget_tab_surfaces(&mut self, id: &str) {
        self.panels.forget(id);
        self.agent_panel_seen.remove(id);
        self.convo.forget(id);
        self.dev_forget(id);
        self.s7.notifier.forget(id);
        self.web.close_tab(id);
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
                    self.tab_picked();
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
                    self.tab_picked();
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
        self.plugins.open = false;
        self.refresh();
    }

    // ---- menus, the +N list and the start screen -------------------------------------------------------

    /// The strip's + right-clicked: the folder menu at the pointer.
    fn open_plus_menu(&mut self, x: f32, y: f32) {
        self.close_overflow_quietly();
        let rows = plus_menu_rows(&self.prefs, &self.starters, &self.deps.home);
        self.show_menu(MenuKind::Plus, rows, x, y);
    }

    /// The sidebar New tab button's arrow: the same menu as a dropdown from the button (`right` is its right edge,
    /// `top` and `foot` its top and foot). It opens upward, right-aligned to the arrow, and never covers the button.
    fn open_folder_menu(&mut self, right: f32, top: f32, foot: f32) {
        self.close_overflow_quietly();
        let rows = plus_menu_rows(&self.prefs, &self.starters, &self.deps.home);
        self.show_menu_at(MenuKind::Plus, rows, right, top, Some(foot));
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
        let shell = tab.kind == TabKind::Shell;
        let mut rows: Vec<(MenuRowData, String)> =
            tab_menu_rows(&cwd, self.key_label_of(ChordId::CloseTab))
                .into_iter()
                .map(|r| (r, String::new()))
                .collect();
        // The plugins' actions for a shell tab, one flyout under each plugin's name (plugins phase 4).
        if shell {
            self.plugin_menu_rows(id, &mut rows);
        }
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
        self.show_menu_at(kind, rows, x, y, None);
    }

    /// `drop`: a dropdown from a button whose foot is that `y` (the sidebar's New tab arrow); `x` is then the
    /// button's right edge and `y` its top. Without it the menu's top-left is at the point.
    fn show_menu_at(
        &mut self,
        kind: MenuKind,
        rows: Vec<(MenuRowData, String)>,
        x: f32,
        y: f32,
        drop: Option<f32>,
    ) {
        self.clear_tip();
        self.term.hide_thumb();
        self.menu = Some(MenuState { kind, rows });
        if let Some(app) = self.app() {
            app.set_menu_x(x);
            app.set_menu_y(y);
            app.set_menu_drop(drop.is_some());
            app.set_menu_foot(drop.unwrap_or(y));
        }
        self.push_menu();
        if let Some(app) = self.app() {
            // Every open, a reopen included (see ContextMenu::shown).
            app.invoke_menu_shown();
        }
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

    /// A right-click released outside an open menu, the +N list or a Settings dropdown: all close, and
    /// the same right-click is handed to whatever is under the point, so one click closes this menu and
    /// opens the next (a scrim swallows the press, so the surface beneath never saw it). The
    /// hand-over is Slint's own event path (`Window::dispatch_event`), no OS input, so every
    /// surface decides what a right-click means exactly as for a real one (a program that owns the
    /// mouse still gets the report, rule 156). It runs from a zero timer: the real release is
    /// still being delivered to the scrim, and the controller is borrowed for it.
    fn outside_right_click(&mut self, x: f32, y: f32) {
        self.close_menu_quietly();
        self.plugins.open = false;
        let overflow_was_open = self.overflow.is_some();
        self.close_overflow_quietly();
        self.refresh();
        // The hidden +N query field must not keep the keys; a surface that takes focus (a menu, the
        // terminal) still takes it in the dispatched press.
        if overflow_was_open {
            self.focus_terminal();
        }
        hand_on_right_click(self.weak.clone(), x, y);
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
                // The first row is always "Choose a folder…"; by its place, as a folder may carry that name.
                if i == 0 {
                    self.choose_folder();
                } else if !path.is_empty() {
                    self.open_recent(path);
                }
            }
            MenuKind::Tab { id, .. } if self.plugin_menu_picked(&id.clone(), &path.clone()) => {}
            MenuKind::Section { sid } => self.section_menu_picked(&sid.clone(), &row.label.clone()),
            MenuKind::Staged => self.staged_menu_picked(&row.label.clone()),
            MenuKind::Tab { id, cwd } => match row.label.as_str() {
                "Close tab" => self.request_close(id),
                "Copy folder path" => (self.deps.clipboard)(cwd),
                _ => {}
            },
            MenuKind::Term { tab } if row.label == ASK_AGENT_ROW => {
                let selection = self.term.menu_selection().map(str::to_string);
                self.term.on_menu_pick(None, usize::MAX);
                self.ask_about_grid(tab, selection.as_deref().unwrap_or(""));
            }
            MenuKind::Term { tab } if self.convo_menu_picked(tab, &row.label) => {
                // One of the conversation rows: the terminal's own menu state is let go, nothing else happens.
                self.term.on_menu_pick(None, usize::MAX);
            }
            MenuKind::Term { .. } => {
                if row.label == "Command help" {
                    self.help_toggle();
                }
                let front = self.front_tab();
                {
                    let info = front.as_ref().map(FrontTab::info);
                    let idx = usize::try_from(i).unwrap_or(usize::MAX);
                    self.term.on_menu_pick(info.as_ref(), idx);
                }
                self.push_term();
                self.wake_term();
            }
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
            toggle_pin_in_menu(&self.prefs, &mut m.rows, i, &self.starters, &self.deps.home);
        }
        self.push_menu();
    }

    fn open_overflow(&mut self, all: bool, right: f32, bottom: f32) {
        self.close_menu_quietly();
        self.clear_tip();
        self.term.hide_thumb();
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
                    .map(|r| overflow_row(r, &self.view.rows))
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
        self.tab_picked();
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
        self.tip_request_toward(text, x, y, w, h, None);
    }

    /// A tooltip on the side `side` asks for (still flipped when there is no room), or where tips usually go.
    fn tip_request_toward(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        side: Option<TipSide>,
    ) {
        if text.is_empty() {
            self.clear_tip();
            return;
        }
        let now = self.now();
        let side_width = self.sidebar.view(self.scale, now).width_css as f32;
        let prefer = if let Some(side) = side {
            side
        } else if self.vertical() && x < side_width {
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

    // ---- the terminal's input (M5) ----------------------------------------------------------------------

    /// The shell tab in front, as owned pieces. `alive` is a shell that was spawned and is running.
    fn front_tab(&self) -> Option<FrontTab> {
        let t = self.model.active_shell()?;
        let handle = self.deps.host.handle(&t.id)?;
        let alive = handle.shell_id().is_some();
        Some(FrontTab {
            id: t.id.clone(),
            handle,
            cwd: (!t.cwd.is_empty()).then(|| t.cwd.clone()),
            alive,
        })
    }

    /// The grid's cells in the pointer's units: the live metrics, the window's scale, the size of
    /// the tab in front.
    fn grid_geom(&self, front: Option<&FrontTab>) -> GridGeom {
        let scale = self.app().map_or(self.scale, |a| f64::from(a.get_scale()));
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let (cols, rows) = front.map_or(self.grid_dims, |f| {
            f.handle.with_emu_quiet(|e| (e.cols(), e.view_rows()))
        });
        let window = self.app().map_or((0, 0), |a| {
            let s = a.window().size();
            (s.width, s.height)
        });
        GridGeom {
            metrics: (self.deps.term.metrics)(),
            scale: scale as f32,
            cols,
            rows,
            window,
        }
    }

    /// The timer must run once: the terminal has something to look at.
    fn wake_term(&mut self) {
        self.term_due = Some(self.now());
        self.kick();
    }

    /// Pushes the find bar, the badge and the focus that follows the bar to the window.
    fn push_term(&mut self) {
        let Some(app) = self.app() else { return };
        let fs = self.term.find_state();
        app.set_find_open(fs.open);
        if app.get_find_query().as_str() != fs.query {
            app.set_find_query(fs.query.as_str().into());
        }
        app.set_find_readout(fs.readout.as_str().into());
        app.set_find_can_step(fs.can_step);
        app.set_copied_phase(self.term.badge_phase().code());
        if fs.open && !self.find_shown {
            app.invoke_focus_find();
        } else if !fs.open && self.find_shown {
            app.invoke_focus_terminal();
        }
        if fs.open
            && !self.find_shown
            && let Some(h) = self.help.clone()
            && let Ok(mut h) = h.try_borrow_mut()
        {
            h.note_find_opened(next_open_seq());
        }
        self.find_shown = fs.open;
    }

    /// The terminal's share of the timer: the drag scroll, the badge, the find job, the path
    /// questions and the drop batch.
    fn term_tick(&mut self) {
        let front = self.front_tab();
        let g = self.grid_geom(front.as_ref());
        let out = {
            let info = front.as_ref().map(FrontTab::info);
            self.term.tick(info.as_ref(), &g)
        };
        self.term_due = out.next_due_ms;
        self.term_hot = self.term_hot.saturating_sub(1);
        // Start-up trace: the first tick in which the front grid shows a non-empty row.
        if !crate::startup_trace::is_marked(crate::startup_trace::Phase::FirstPrompt)
            && self.front_shows_prompt()
        {
            crate::startup_trace::mark(crate::startup_trace::Phase::FirstPrompt);
        }
        if out.focus_terminal {
            self.focus_terminal();
        }
        self.push_term();
    }

    /// One pointer event over the grid (css px from the grid's corner).
    pub fn grid_pointer(&mut self, e: PointerIn) {
        let front = self.front_tab();
        let g = self.grid_geom(front.as_ref());
        let out = {
            let info = front.as_ref().map(FrontTab::info);
            self.term.on_pointer(info.as_ref(), &g, e)
        };
        if let Some(at) = out.open_menu {
            self.open_term_menu(at);
        }
        if out.focus_terminal {
            self.focus_terminal();
        }
        self.push_term();
        self.wake_term();
    }

    /// The pointer left the grid: a hover thumbnail goes, and the tick has one less reason to run.
    fn grid_pointer_left(&mut self) {
        self.term.pointer_left();
    }

    /// The terminal's right-click menu, hung where the pointer is (the grid's corner is added).
    fn open_term_menu(&mut self, at: crate::term_menu::TermMenuAt) {
        self.close_overflow_quietly();
        let find_label = self.key_label_of(ChordId::Find);
        let help_label = self.key_label_of(ChordId::Help);
        let help_on = flare_settings::help_prefs::help_enabled(&self.prefs);
        let mut rows: Vec<(MenuRowData, String)> = self
            .term
            .menu_rows(help_on, find_label, help_label)
            .iter()
            .map(|r| (to_menu_row(r), String::new()))
            .collect();
        if rows.is_empty() {
            return;
        }
        rows.extend(self.ask_menu_rows(&at.tab));
        rows.extend(self.convo_menu_rows(&at.tab));
        let (gx, gy) = self
            .app()
            .map_or((0.0, 0.0), |a| (a.get_grid_left(), a.get_grid_top()));
        self.show_menu(MenuKind::Term { tab: at.tab }, rows, at.x + gx, at.y + gy);
    }

    /// Ctrl+F: opens the find bar on the tab in front, or closes it there (a full-screen program
    /// keeps Ctrl+F; the chord table decided that already).
    fn toggle_find(&mut self) {
        let Some(front) = self.front_tab() else {
            return;
        };
        self.term.toggle_find(&front.info());
        self.push_term();
        self.wake_term();
    }

    fn term_find_edited(&mut self, q: &str) {
        if let Some(front) = self.front_tab() {
            self.term.find_edited(&front.info(), q);
        }
        self.push_term();
        self.wake_term();
    }

    fn term_find_step(&mut self, forward: bool) {
        if let Some(front) = self.front_tab() {
            if forward {
                self.term.find_next(&front.info());
            } else {
                self.term.find_prev(&front.info());
            }
        }
        self.push_term();
        self.wake_term();
    }

    fn term_find_close(&mut self) {
        self.term.find_close();
        self.push_term();
        self.focus_terminal();
    }

    /// A key no chord took, for the shell in front: the terminal's rules decide (copy, delete over
    /// a selection, paste, the agent's newline), else the shell gets it.
    fn term_key(&mut self, ev: &KeyEvent, ctx: &KeyContext) -> KeyOutcome {
        let Some(front) = self.front_tab() else {
            return KeyOutcome::Consumed;
        };
        let info = front.info();
        let has_selection = self.term.has_selection(&info);
        let agent_here = self.agent_view.armed(&front.id).is_some();
        let action = {
            let term = &mut self.term;
            let mut delete = || term.selection_delete(&info);
            let host = |e: &KeyEvent| app_owns_key(e, ctx);
            let mut state = KeyState {
                has_selection,
                delete_selection: &mut delete,
                host_owns: &host,
                agent_here,
            };
            term_key_action(ev, &mut state)
        };
        let done = self.term.on_key(&info, ev, action);
        self.push_term();
        self.wake_term();
        match done {
            KeyDone::Handled => KeyOutcome::Consumed,
            KeyDone::Send => KeyOutcome::Pty,
        }
    }

    // ---- the panels (Surfaces stage 1) --------------------------------------------------------------

    /// The shell tabs as (id, folder), in the strip's order: what the panels are stored against.
    fn shell_tabs(&self) -> Vec<(String, String)> {
        flare_tabs::tabs::shell_tabs(&self.model.state)
            .iter()
            .map(|t| (t.id.clone(), t.cwd.clone()))
            .collect()
    }

    /// The limits of the slot, from the layout maths, handed to the window once. The `.slint` file
    /// evaluates the same formulas in logical pixels; a test pins the two to each other.
    pub(crate) fn push_panel_limits(app: &AppWindow) {
        use flare_surfaces::layout as l;
        app.set_panel_min_slot(l::MIN_SLOT as f32);
        app.set_panel_min_grid(l::MIN_GRID as f32);
        app.set_panel_usable_min(l::USABLE_MIN as f32);
        app.set_panel_max_share(l::MAX_SHARE as f32);
        app.set_panel_default_share(l::DEFAULT_SHARE);
    }

    /// How many tabs have a panel open (the `panel.state` probe).
    pub fn open_panels(&self) -> usize {
        self.panels.open_count()
    }

    /// Hands the panels the store (startup, before the restore).
    pub fn set_panel_store(&mut self, store: Arc<flare_surfaces::store::PanelStore>) {
        self.panels.set_store(store);
    }

    /// The restore made the tabs: each takes the panel its folder and place name.
    fn restore_panels(&mut self) {
        let tabs = self.shell_tabs();
        self.panels.restore(&tabs);
    }

    /// The front tab's markdown page could not be shown (its browser failed: no WebView2 runtime, a renderer that
    /// died): the file shows in Code instead, at once (rule 240).
    fn md_page_fallback(&mut self, app: &AppWindow) {
        let Some(front) = self.model.active_shell().map(|t| t.id.clone()) else {
            return;
        };
        if self.web.failure(&front).is_none() || !self.panels.page_failed(&front) {
            return;
        }
        self.panels.sync(app, Some(&front));
        let ctx = self.key_context();
        self.web.sync(app, &self.panels, Some(&front), ctx);
    }

    /// The window shows the front tab's panel; the keys go back to the terminal when the panel that
    /// had them goes away.
    fn sync_panels(&mut self, app: &AppWindow) {
        let front = self.model.active_shell().map(|t| t.id.clone());
        // Another tab came to the front (a chord) while the splitter was held: that tab's slot may be on show, so
        // `panel-shown` never dropped, but the drag was for the old one. Let go before its panel is pushed.
        if self.panel_front_seen != front {
            self.panel_front_seen.clone_from(&front);
            app.set_panel_splitting(false);
            // The keys were in the old tab's panel. The new tab may show a panel too (so `panel-shown` never
            // drops), and a panel takes no plain key: the keys are the terminal's now.
            if app.get_panel_has_focus() {
                self.panel_focus_seen = false;
                app.invoke_focus_terminal();
            }
        }
        self.panels
            .set_wrap(flare_settings::term_prefs::wrap_previews(&self.prefs));
        self.panels.sync(app, front.as_deref());
        app.set_panel_ask_on(front.as_deref().is_some_and(|t| self.hub.agent_here(t)));
        // The bar's height first: the terminal is laid out shorter before the browser is placed.
        self.push_bar(app);
        let ctx = self.key_context();
        self.web.sync(app, &self.panels, front.as_deref(), ctx);
        self.md_page_fallback(app);
        self.sync_bar_views(app);
        self.sync_web_full();
        self.sync_convo();
        let kind = front
            .as_deref()
            .and_then(|t| self.panels.open_convo_kind(t).map(|k| (t, k)));
        self.convo.push(app, kind.as_ref().map(|(t, k)| (*t, k)));
        self.sync_chip();
        self.sync_mods();
        self.prune_downloads();
        self.prune_app_asks();
        let had = self.panel_focus_seen;
        self.panel_focus_seen = app.get_panel_has_focus();
        if had && !app.get_panel_shown() {
            self.panel_focus_seen = false;
            app.invoke_focus_terminal();
        }
    }

    // ---- the conversation panels (Surfaces stage 6) ------------------------------------------------

    /// Brings the transcript trackers in line with the open Changes panels: one per such tab whose Claude
    /// session is known, none otherwise. True when one started or stopped (the window follows).
    fn sync_convo(&mut self) -> bool {
        let open = self.panels.open_convo();
        if open.is_empty() && !self.convo.active() {
            return false;
        }
        let shells = self.shell_tabs();
        let wants: Vec<Want> = open
            .iter()
            .filter_map(|(tab, _)| {
                let folder = shells.iter().find(|s| &s.0 == tab)?.1.clone();
                Some(Want {
                    tab: tab.clone(),
                    folder,
                    session: self.hub.claude_session(tab).map(str::to_string),
                })
            })
            .collect();
        self.convo.sync(&wants)
    }

    /// The conversation trackers, read only (the e2e probe, the tests).
    pub(crate) fn convo(&self) -> &ConvoCtl {
        &self.convo
    }

    /// Reads a Claude folder of the test's own instead of the profile's.
    #[cfg(test)]
    pub(crate) fn set_convo_claude_dir(&mut self, dir: Option<PathBuf>) {
        self.convo.set_claude_dir(dir);
    }

    /// A tracker's snapshot, or a clicked path that may be shown.
    fn on_convo_msg(&mut self, m: ConvoMsg) {
        match m {
            ConvoMsg::Snap { tab, session, snap } => {
                if self.convo.on_snap(&tab, &session, snap) {
                    // Only the front tab's panel is on show; a snapshot of another tab just waits for it.
                    if self.front_id().as_deref() == Some(tab.as_str()) {
                        self.refresh();
                    }
                }
            }
            ConvoMsg::OpenFile {
                tab,
                kind,
                abs,
                picked,
            } => {
                // The person asked for this file by clicking its path: the panel shows it, whatever the setting
                // for clicks in the terminal says. The Changes panel closes (a tab has one panel). An answer that
                // arrives after the person moved on (another panel, another pick) is dropped.
                let still = self.panels.open_convo_kind(&tab)
                    == Some(flare_surfaces::PanelKind::Changes)
                    && self.convo.selected(&tab) == picked;
                if still && self.is_shell_tab(&tab) && self.panels.show(&tab, kind, &abs) {
                    self.refresh();
                    self.panel_changed(&tab);
                }
            }
        }
    }

    /// A file picked in the Changes panel of the front tab (an empty path goes back to the list).
    fn convo_select(&mut self, path: &str) {
        let Some(id) = self.front_id() else { return };
        let pick = (!path.is_empty()).then(|| path.to_string());
        if self.convo.select(&id, pick) {
            self.refresh();
        }
    }

    /// The path in a diff was clicked: a worker re-resolves it (rule 13) and, when it is a picture or a text file on
    /// the click allowlist (rule 14), the panel shows it. Anything else does nothing; nothing is ever run.
    fn convo_open(&mut self, path: &str) {
        let Some(id) = self.front_id() else { return };
        if path.is_empty() || path.len() > 4096 || path.contains('\0') {
            return;
        }
        let folder = self
            .shell_tabs()
            .into_iter()
            .find(|t| t.0 == id)
            .map(|t| t.1)
            .unwrap_or_default();
        let post = Arc::clone(&self.deps.post);
        let home = self.deps.home.clone();
        let path = path.to_string();
        let picked = self.convo.selected(&id);
        let spawned = std::thread::Builder::new()
            .name("flare-convo-open".into())
            .spawn(move || {
                if let Some((abs, real)) =
                    flare_term::path_open::resolve_openable_file(&folder, &path, &home)
                    && let Some(kind) = flare_surfaces::preview::classify_resolved(&abs, &real)
                    && flare_surfaces::preview::loadable(&real, &kind)
                {
                    post(UiMsg::Convo(ConvoMsgBox(ConvoMsg::OpenFile {
                        tab: id,
                        kind,
                        abs: real,
                        picked,
                    })));
                }
            });
        if let Err(e) = spawned {
            log::warn!("could not start the path check: {e}");
        }
    }

    /// The row the terminal's menu gains on a tab that runs Claude: one to show or hide the Changes panel. A tab
    /// without Claude gets none, so the plain shell's menu does not change.
    pub(crate) fn convo_menu_rows(&self, tab: &str) -> Vec<(MenuRowData, String)> {
        let claude = self.hub.agent_here(tab)
            && (self.hub.claude_session(tab).is_some()
                || self.hub.agent_kind(tab) == Some(flare_agent::kinds::DetectedAgent::Claude));
        if !claude {
            return Vec::new();
        }
        let open = self.panels.open_convo_kind(tab);
        let changes_open = open == Some(flare_surfaces::PanelKind::Changes);
        // The gate above proves it is Claude (a session, or the detection), so the row always names Claude.
        let changes = if changes_open {
            CONVO_HIDE_CHANGES
        } else {
            CONVO_SHOW_CHANGES
        };
        // Its own group, after a rule: it is about the agent, not the terminal.
        vec![agent_menu_row(changes, crate::menus_ui::GLYPH_CHANGES)]
    }

    /// The row the terminal's menu gains over a grid selection on a tab with an agent, whichever agent it is.
    pub(crate) fn ask_menu_rows(&self, tab: &str) -> Vec<(MenuRowData, String)> {
        if self.hub.agent_here(tab) && self.term.menu_selection().is_some() {
            vec![agent_menu_row(ASK_AGENT_ROW, "menu-paste")]
        } else {
            Vec::new()
        }
    }

    /// A row of the convo part of the terminal's menu was picked. False for any other row.
    pub(crate) fn convo_menu_picked(&mut self, tab: &str, label: &str) -> bool {
        use flare_surfaces::PanelKind;
        let (kind, show) = match label {
            CONVO_SHOW_CHANGES => (PanelKind::Changes, true),
            CONVO_HIDE_CHANGES => (PanelKind::Changes, false),
            _ => return false,
        };
        if show {
            self.open_convo_panel(tab, kind);
        } else if self.panels.close(tab) {
            self.refresh();
            self.panel_changed(tab);
        }
        true
    }

    /// Opens the Changes panel on the tab (the menu, and `panel.open` from the agent). False when the tab
    /// cannot hold one.
    pub(crate) fn open_convo_panel(&mut self, tab: &str, kind: flare_surfaces::PanelKind) -> bool {
        if !self.is_shell_tab(tab) || !self.panels.show_convo(tab, kind) {
            return false;
        }
        self.refresh();
        self.panel_changed(tab);
        true
    }

    /// A click on a path that is a picture or a text file: the backend resolved it and a worker
    /// said what it may be shown as. The panel opens on that tab; the keys stay where they are.
    pub(crate) fn show_preview(&mut self, tab: &str, kind: flare_surfaces::PanelKind, abs: &str) {
        let is_shell = flare_tabs::tabs::shell_tabs(&self.model.state)
            .iter()
            .any(|t| t.id == tab);
        if !is_shell || !flare_settings::term_prefs::open_previews(&self.prefs) {
            return;
        }
        // A window too narrow for a slot would open a panel nobody can see, and the click would look dead: the file
        // goes the way a click goes with previews off.
        if self.app().is_some_and(|a| !a.get_panel_fits()) {
            let (openers, home) = (Arc::clone(&self.deps.term.openers), self.deps.home.clone());
            let path = std::path::Path::new(abs);
            let base = path.parent().map(|p| p.to_string_lossy().into_owned());
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if let (Some(base), Some(name)) = (base, name) {
                std::thread::spawn(move || {
                    let _ = flare_term::path_open::open_term_path(
                        &base,
                        &name,
                        flare_term::path_open::OpenMode::Open,
                        &*openers,
                        &home,
                    );
                });
            }
            return;
        }
        self.show_preview_asked(tab, kind, abs);
    }

    /// The person asked for it (the menu's "Show in side panel"): the same panel, whatever "Open previews in Flare"
    /// says about plain clicks.
    pub(crate) fn show_preview_asked(
        &mut self,
        tab: &str,
        kind: flare_surfaces::PanelKind,
        abs: &str,
    ) {
        if !self.is_shell_tab(tab) {
            return;
        }
        if self.panels.show(tab, kind, abs) {
            self.refresh();
            self.panel_changed(tab);
        }
    }

    // ---- the control channel (Surfaces stage 2) -------------------------------------------------------------------------

    /// Where events go, and what the channel is called (startup).
    pub fn set_ctl(&mut self, events: CtlEvents, info: CtlInfo) {
        self.ctl_events = Some(events);
        self.ctl_info = Some(info);
    }

    /// The pipe and the MCP server's path of this run, for the e2e probe.
    pub fn ctl_info(&self) -> Option<CtlInfo> {
        self.ctl_info.clone()
    }

    /// The channel's pipe and every shell tab's id and secret, for the `ctl.info` e2e probe (the
    /// scenario plays the shell). Nothing else reads a tab's secret.
    pub fn ctl_probe(&self) -> serde_json::Value {
        let mgr = self.deps.host.manager();
        let active = self.model.state.active_id.clone();
        let tabs: Vec<serde_json::Value> = flare_tabs::tabs::shell_tabs(&self.model.state)
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "active": active.as_deref() == Some(&t.id),
                    "token": mgr.ctl_token(&t.id),
                })
            })
            .collect();
        let info = self.ctl_info.clone().unwrap_or_default();
        serde_json::json!({"pipe": info.pipe, "pid": std::process::id(), "mcp_exe": info.mcp_exe, "tabs": tabs})
    }

    /// `PanelInfo` of a tab's panel, as the protocol writes it.
    fn panel_info(snap: &crate::panel_ctl::PanelSnap) -> serde_json::Value {
        let mut v = serde_json::json!({"kind": snap.kind, "title": snap.title});
        if let Some(p) = &snap.path {
            v["path"] = serde_json::json!(p);
        }
        if let Some(u) = &snap.url {
            v["url"] = serde_json::json!(u);
        }
        if let Some(n) = snap.network {
            v["network"] = serde_json::json!(n);
        }
        v
    }

    /// Tells the tab's connections its panel changed (`panel.changed`).
    fn panel_changed(&self, tab: &str) {
        let Some(events) = &self.ctl_events else {
            return;
        };
        let mut params = serde_json::json!({"tab": tab, "open": false});
        if let Some(snap) = self.panels.snap(tab).filter(|s| s.open) {
            params["open"] = serde_json::json!(true);
            params["panel"] = Self::panel_info(&snap);
        }
        events("panel.changed", params, Some(tab));
    }

    /// Whether a tab holds any badge or progress (tests).
    #[cfg(test)]
    pub(crate) fn has_decor(&self, tab: &str) -> bool {
        self.decor.get(tab).is_some()
    }

    /// The agent's badges and progress leave every tab whose session changed or whose agent left (the hub says
    /// which). True when a tab lost something it showed.
    fn clear_agent_decor(&mut self) -> bool {
        let mut changed = false;
        for (tab, _why) in self.hub.take_decor_gone() {
            changed |= self.decor.clear_source(&tab, &DecorSource::Agent);
        }
        changed
    }

    /// The agent's badges and progress need an agent in the tab: its token alone is not enough, since any
    /// process in a plain shell tab holds it. The decor would wear an agent's name no one runs, and nothing
    /// would ever clear it (the hub reports an agent leaving only where one was).
    fn agent_decor_allowed(
        &self,
        tab: &str,
        source: &DecorSource,
        generation: u64,
    ) -> Result<(), flare_ctl::CtlError> {
        if *source == DecorSource::Agent && self.hub.agent_kind(tab).is_none() {
            return Err(flare_ctl::CtlError::new(
                flare_ctl::code::FORBIDDEN,
                "no agent in this tab",
            ));
        }
        // A plugin's change that was checked while it was on is not applied once it is off: everything it put up
        // went when it was turned off, and nothing of it comes back, not even when it is on again by now (its
        // generation moved on).
        if let DecorSource::Plugin(id) = source
            && self.mods.as_ref().is_some_and(|m| {
                m.hub().grant_of_mod(id).is_none() || m.hub().generation(id) != generation
            })
        {
            return Err(flare_ctl::CtlError::new(
                flare_ctl::code::UNAUTHORIZED,
                "the plugin is turned off",
            ));
        }
        Ok(())
    }

    fn is_shell_tab(&self, tab: &str) -> bool {
        flare_tabs::tabs::shell_tabs(&self.model.state)
            .iter()
            .any(|t| t.id == tab)
    }

    /// One call of the control channel, on the UI thread. The tab is the caller's own (resolved from
    /// its token by the worker); a tab that was closed since answers `no_tab`.
    fn ctl_op(&mut self, op: CtlOp) -> Result<serde_json::Value, flare_ctl::CtlError> {
        use flare_ctl::{CtlError, code};
        use serde_json::json;
        let no_tab = || CtlError::new(code::NO_TAB, "that tab is closed");
        match op {
            CtlOp::Tabs { own } => {
                let active = self.model.state.active_id.clone();
                let tabs: Vec<serde_json::Value> = self
                    .model
                    .state
                    .tabs
                    .iter()
                    .zip(self.model.labels())
                    .filter(|(t, _)| {
                        t.kind == TabKind::Shell && own.as_deref().is_none_or(|o| o == t.id)
                    })
                    .map(|(t, label)| {
                        json!({"id": t.id, "title": label, "active": active.as_deref() == Some(&t.id)})
                    })
                    .collect();
                Ok(json!({"tabs": tabs}))
            }
            CtlOp::TabsWide => {
                let tabs: Vec<serde_json::Value> =
                    self.tab_facts().iter().map(|f| f.to_json()).collect();
                Ok(json!({"tabs": tabs}))
            }
            CtlOp::TabCwd { tab } => {
                let t = flare_tabs::tabs::shell_tabs(&self.model.state)
                    .into_iter()
                    .find(|t| t.id == tab)
                    .ok_or_else(no_tab)?;
                Ok(json!({"cwd": t.cwd}))
            }
            CtlOp::PanelState { tab } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                Ok(match self.panels.snap(&tab).filter(|s| s.open) {
                    Some(s) => {
                        let mut v = json!({
                            "open": true,
                            "panel": Self::panel_info(&s),
                            "width": s.width.unwrap_or(flare_surfaces::layout::DEFAULT_SHARE),
                        });
                        // A pop-up over the page is the person's to finish (rule 250): `browser_state` says so.
                        if self.web.popup_open(&tab) {
                            v["popup"] = json!(true);
                        }
                        v
                    }
                    None => json!({"open": false}),
                })
            }
            CtlOp::ShowConvo { tab, kind } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                // A tab behind gets its panel (stored) and keeps its place: no tab is ever switched.
                if !self.open_convo_panel(&tab, kind) {
                    return Err(CtlError::new(
                        code::INTERNAL,
                        "the panel could not be opened",
                    ));
                }
                self.mark_agent_panel(&tab);
                let snap = self
                    .panels
                    .snap(&tab)
                    .ok_or_else(|| CtlError::new(code::INTERNAL, "the panel went missing"))?;
                Ok(json!({"panel": Self::panel_info(&snap)}))
            }
            CtlOp::ConvoState { tab } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                Ok(self.convo.state_json(&tab))
            }
            CtlOp::PanelClose { tab } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                let closed = self.panels.close(&tab);
                if closed {
                    self.refresh();
                    self.panel_changed(&tab);
                }
                Ok(json!({"closed": closed}))
            }
            CtlOp::ShowFile {
                tab,
                kind,
                abs,
                title,
            } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                // An explicit ask of the agent: the "Open previews in Flare" switch is for clicks only.
                // A tab behind gets its panel (stored) and keeps its place: no tab is ever switched.
                if !self.panels.show_titled(&tab, kind, &abs, title.as_deref()) {
                    return Err(CtlError::new(code::BAD_PARAMS, "that file cannot be shown"));
                }
                self.mark_agent_panel(&tab);
                self.after_ctl_show(&tab)
            }
            CtlOp::ShowText { tab, text, title } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                if !self.panels.show_text(&tab, &text, title.as_deref()) {
                    return Err(CtlError::new(code::INTERNAL, "the text could not be shown"));
                }
                self.mark_agent_panel(&tab);
                self.after_ctl_show(&tab)
            }
            // Decor (plugins phase 1): the worker checked every param and the rate; the tab keeps it and is drawn
            // again only when something a person sees changed.
            CtlOp::BadgeSet {
                tab,
                source,
                key,
                icon,
                tip,
                tone,
                generation,
            } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                self.agent_decor_allowed(&tab, &source, generation)?;
                match self.decor.set_badge(&tab, source, &key, &icon, &tip, tone) {
                    Ok(changed) => {
                        if changed {
                            self.refresh();
                        }
                        Ok(json!({}))
                    }
                    Err(Refused::TooManyKeys) => Err(CtlError::new(
                        code::BAD_PARAMS,
                        format!(
                            "a caller keeps at most {} badges on one tab and {} in all; clear one first",
                            flare_tabs::decor::MAX_KEYS_PER_TAB,
                            flare_tabs::decor::MAX_KEYS_TOTAL
                        ),
                    )),
                }
            }
            CtlOp::BadgeClear { tab, source, key } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                let cleared = self.decor.clear_badge(&tab, &source, &key);
                if cleared {
                    self.refresh();
                }
                Ok(json!({"cleared": cleared}))
            }
            CtlOp::ProgressSet {
                tab,
                source,
                key,
                value,
                label,
                generation,
            } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                self.agent_decor_allowed(&tab, &source, generation)?;
                if self.decor.set_progress(&tab, source, &key, value, &label) {
                    self.refresh();
                }
                Ok(json!({}))
            }
            CtlOp::ProgressClear { tab, source, key } => {
                if !self.is_shell_tab(&tab) {
                    return Err(no_tab());
                }
                let cleared = self.decor.clear_progress(&tab, &source, &key);
                if cleared {
                    self.refresh();
                }
                Ok(json!({"cleared": cleared}))
            }
            // The plugins' sections and actions (plugins phase 4): checked on the worker, applied in `plugins_glue`.
            CtlOp::SidebarSet {
                plugin,
                key,
                items,
                generation,
            } => self.ctl_sidebar_set(plugin, key, items, generation),
            CtlOp::ActionsSet {
                plugin,
                key,
                tab,
                state,
                generation,
            } => self.ctl_actions_set(plugin, key, tab, state, generation),
            // Sessions and typing (plugins phase 5): checked on the worker, applied in `sessions_glue`.
            CtlOp::SessionOf { tab } => self.ctl_session_of(&tab),
            CtlOp::TabInput {
                plugin,
                tab,
                text,
                enter,
                generation,
            } => self.ctl_tab_input(plugin, tab, text, enter, generation),
            // The session bar (plugins phase 6): checked on the worker, applied in `bar_glue`.
            CtlOp::BarShow {
                plugin,
                tab,
                generation,
            } => self.ctl_bar_show(plugin, tab, generation),
            CtlOp::BarHide { plugin, tab } => self.ctl_bar_hide(plugin, tab),
            // The agents' icon motions (owner 2026-10-26): checked on the worker, applied in `plugins_glue`.
            CtlOp::MotionSet {
                plugin,
                agent,
                motion,
                generation,
            } => self.ctl_motion_set(plugin, agent, motion, generation),
            CtlOp::MotionClear { plugin, agent } => self.ctl_motion_clear(plugin, agent),
            // The address of a page to fill (the `fill` permission), applied in `ask_glue`.
            CtlOp::FillPage { tab } => self.ctl_fill_page(&tab),
            // Answered by the browser's message, never here (see `ctl_browser`), or by the person (`ask_glue`).
            CtlOp::BrowserOpen { .. }
            | CtlOp::BrowserNavigate { .. }
            | CtlOp::BrowserAgent { .. }
            | CtlOp::ShowMod { .. }
            | CtlOp::ModTool { .. }
            | CtlOp::Fill { .. }
            | CtlOp::SecretAsk { .. }
            | CtlOp::ShowApp { .. } => Err(CtlError::new(
                code::INTERNAL,
                "a browser call reached the wrong handler",
            )),
        }
    }

    /// A panel was put on a tab by the control channel: the window follows, the event goes out and the
    /// answer says what is on show.
    fn after_ctl_show(&mut self, tab: &str) -> Result<serde_json::Value, flare_ctl::CtlError> {
        self.refresh();
        self.panel_changed(tab);
        let snap = self.panels.snap(tab).ok_or_else(|| {
            flare_ctl::CtlError::new(flare_ctl::code::INTERNAL, "the panel went missing")
        })?;
        Ok(serde_json::json!({"panel": Self::panel_info(&snap)}))
    }

    /// The panel's close button, and the toggle chord when it is open.
    fn close_front_panel(&mut self) {
        let Some(id) = self.front_id() else { return };
        if self.panels.close(&id) {
            self.refresh();
            self.focus_terminal();
            self.panel_changed(&id);
        }
    }

    /// Ctrl+Shift+P: closes the front tab's panel, or brings back what it last showed. False when
    /// there is neither, so the key is not taken.
    pub(crate) fn toggle_front_panel(&mut self) -> bool {
        let Some(id) = self.front_id() else {
            return false;
        };
        // A kind this build cannot draw (restored from the file) is kept as stored, and the chord
        // does not flip it: nothing would appear or go.
        if !self.panels.has_built(&id) {
            return false;
        }
        let changed = if self.panels.is_open(&id) {
            self.panels.close(&id)
        } else {
            self.panels.reopen(&id)
        };
        if changed {
            self.refresh();
            if !self.panels.is_open(&id) {
                self.focus_terminal();
            }
            self.panel_changed(&id);
        }
        changed
    }

    /// Ctrl+Shift+O: the keys move between the terminal and the panel. False when no panel shows.
    fn move_panel_focus(&mut self) -> bool {
        let Some(app) = self.app() else { return false };
        if !app.get_panel_shown() {
            return false;
        }
        // A web page (or a markdown view's page): the keys go to the page, or back to the terminal when it has them.
        if let Some(id) = self.front_id()
            && self.panels.page_target(&id).is_some()
        {
            if self.web.has_focus() {
                self.panel_focus_seen = false;
                self.focus_terminal();
            } else if self.web.focus_page(&id) {
                self.panel_focus_seen = false;
            }
            return true;
        }
        if app.get_panel_has_focus() {
            self.panel_focus_seen = false;
            self.focus_terminal();
        } else {
            self.panel_focus_seen = true;
            app.invoke_focus_panel();
        }
        true
    }

    /// How many grid fits the window reported (the e2e probe reads it).
    pub(crate) fn grid_fit_count(&self) -> u64 {
        self.grid_fits
    }

    /// How many wraps the panel made (the e2e probe and the tests read it).
    pub(crate) fn panel_wrap_count(&self) -> u64 {
        self.panels.wrap_count()
    }

    /// The header's wrap button: flips "Wrap long lines in previews" (the one setting the Settings page shows too).
    fn panel_wrap_toggled(&mut self) {
        let on = flare_settings::term_prefs::wrap_previews(&self.prefs);
        flare_settings::term_prefs::set_wrap_previews(&self.prefs, !on);
        self.panels.set_wrap_moving(false);
        self.refresh();
    }

    /// The header's view control over a markdown file: Compiled, Markdown or Code (0, 1, 2) for the front tab's panel.
    /// The pick stays with the panel (and across a restart); the keys stay where they are.
    fn panel_md_view_picked(&mut self, index: i32) {
        let Some(view) = flare_surfaces::mdview::MdView::from_index(index) else {
            return;
        };
        let Some(tab) = self.front_id() else { return };
        if self.panels.set_md_view(&tab, view) {
            self.panels.set_wrap_moving(false);
        }
        // The pick is stored even when it shows what was on show already (the first pick of the default).
        self.refresh();
    }

    /// The text view holds a different number of columns now (a splitter step, a window edge step, the setting): the
    /// rows wrap to them at once, so the text follows the drag like any editor's (2026-10-23). The terminal grid
    /// still fits once, at the release (rule 24); only the preview's wrap is live. A timer settles the exact final
    /// wrap a moment after the last step.
    fn panel_wrap_cols_changed(&mut self) {
        let Some(app) = self.app() else { return };
        // The sidebar's slide changes the page's width at every frame: the slide's end refresh wraps once (rule 217).
        if app.get_sidebar_sliding() {
            return;
        }
        self.panels.set_wrap_moving(true);
        self.panels.live_wrap(&app);
        let weak = self.me.clone();
        crate::timer_registry::set("panel.wrap-settle", WRAP_SETTLE_MS, false, move || {
            if let Some(rc) = weak.upgrade()
                && let Ok(mut c) = rc.try_borrow_mut()
            {
                c.panels.set_wrap_moving(false);
                if let Some(app) = c.app()
                    && !app.get_sidebar_sliding()
                    && !app.get_panel_splitting()
                {
                    c.panels.settle_wrap(&app);
                }
            }
        });
    }

    /// Esc in the panel: the keys go back to the terminal.
    fn panel_escape(&mut self) {
        self.panel_focus_seen = false;
        self.focus_terminal();
    }

    /// The splitter was let go: the slot's share of the page is the tab's from now on, and the grid
    /// fits once at its new width.
    fn panel_splitter_released(&mut self, share: f32) {
        let Some(id) = self.front_id() else { return };
        self.panels.set_wrap_moving(false);
        if self.panels.set_share(&id, share) {
            if let Some(app) = self.app() {
                app.set_panel_share(share);
            }
            self.refresh();
        }
    }

    // ---- the sidebar's slide (audit V3) ---------------------------------------------------------------------

    /// The window's precise frame clock (fractional ms, the clock `ui_clock` counts on); the tests keep
    /// the controller's own clock.
    pub fn set_frame_clock(&mut self, clock: Rc<dyn Fn() -> f64>) {
        self.frame_clock = clock;
    }

    /// The sidebar's state reached the window: the slot and the panel head for their new rest. A slide
    /// shows its start until its first frame takes the time (`on_frame`).
    fn aim_slide(&mut self, app: &AppWindow, sv: &crate::sidebar_ui::SidebarView) {
        use slint::Global;
        let owed = self.side_slide.aim(&SlideAim {
            collapsed: sv.collapsed,
            revealed: sv.revealed,
            width: sv.width_css,
            sliding: sv.sliding,
            reduced: crate::Motion::get(app).get_reduced(),
        });
        let (slot, panel) = self.side_slide.shown();
        app.set_sidebar_slot_now(slot);
        app.set_sidebar_slide_now(panel);
        // A window with no OS window behind it (the tests, the scenes) has no frames to step the slide:
        // a timer does.
        if owed && !Self::ask_frame(app) {
            self.start_slide_timer();
        }
    }

    /// A frame begins (the window calls this as each frame starts): a slide that has not started starts
    /// now, and the slot and the panel take their place on the curve at this moment. True while the
    /// slide owes more frames.
    pub fn on_frame(&mut self) -> bool {
        if !self.side_slide.running() {
            return false;
        }
        if self.slide_held {
            // A picture is being taken at an offset: the slide waits, and asks for nothing more.
            return false;
        }
        let Some(app) = self.app() else { return false };
        let (slot, panel) = self.side_slide.frame((self.frame_clock)());
        app.set_sidebar_slot_now(slot);
        app.set_sidebar_slide_now(panel);
        let more = self.side_slide.running();
        if !more {
            self.slide_timer.stop();
        }
        more
    }

    /// The e2e's picture at an offset (audit V3): Some(ms) puts a running slide ms into its time and
    /// holds it there, however many frames come, until None lets it go on. A snapshot blocks the UI
    /// thread for 30 to 90 ms on the GPU path, so a picture asked for at an offset otherwise shows wherever
    /// the frames had got to when it finished.
    pub fn hold_slide(&mut self, ms: Option<f64>) {
        let Some(app) = self.app() else { return };
        match ms {
            Some(ms) => {
                let (slot, panel) = self.side_slide.place((self.frame_clock)(), ms.max(0.0));
                app.set_sidebar_slot_now(slot);
                app.set_sidebar_slide_now(panel);
                self.slide_held = true;
            }
            None => {
                self.slide_held = false;
                if self.side_slide.running() {
                    Self::ask_frame(&app);
                }
            }
        }
    }

    /// A sidebar slide waits for its first frame or still moves.
    pub fn slide_running(&self) -> bool {
        self.side_slide.running()
    }

    /// Asks for a frame at once: straight from the OS window when there is one (true), since Slint's
    /// own request waits for its frame timer (whole timer ticks on Windows).
    fn ask_frame(app: &AppWindow) -> bool {
        use slint::winit_030::WinitWindowAccessor;
        let os = app
            .window()
            .with_winit_window(|w| w.request_redraw())
            .is_some();
        if !os {
            app.window().request_redraw();
        }
        os
    }

    /// Steps the slide where no OS window draws frames (the tests, the scenes). Never in the app: a
    /// timer that fired before a slow first frame would take the start from it.
    fn start_slide_timer(&mut self) {
        if self.slide_timer.running() {
            return;
        }
        let weak = self.me.clone();
        self.slide_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(SLIDE_STEP_MS),
            move || {
                if let Some(rc) = weak.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                    && c.on_frame()
                    && let Some(app) = c.app()
                {
                    Self::ask_frame(&app);
                }
            },
        );
    }

    // ---- the timer --------------------------------------------------------------------------------------------

    fn next_due(&self) -> Option<u64> {
        let term = self
            .term_due
            .or_else(|| (self.term_hot > 0).then(|| self.now()));
        // A resuming tab asks to be stepped on every round until its reveal.
        let reveal = self.resuming_any().then(|| self.now());
        [
            self.sidebar.next_due(),
            self.tips.next_due(),
            term,
            self.hub.next_due(),
            self.skeleton_due(),
            self.surf7_next_due(),
            reveal,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Some tab's shell is still resuming (its reveal has not happened).
    fn resuming_any(&self) -> bool {
        self.deps
            .host
            .live_ids()
            .iter()
            .any(|id| self.deps.host.handle(id).is_some_and(|h| h.resuming()))
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
                    crate::diag_ctl::timed("ctl_tick", || c.tick());
                }
            },
        );
        self.timer_on = true;
    }

    /// The timer's body; stops the timer when nothing is pending.
    pub fn tick(&mut self) {
        let now = self.now();
        if let Some(app) = self.app() {
            self.on_minimised(app.window().is_minimized());
        }
        let events = self.sidebar.tick(now);
        let mut changed = !events.is_empty();
        for ev in events {
            if ev == SidebarEvent::FitDue
                && let Some(app) = self.app()
            {
                // One fit at the end of the slide (D9): the grid takes its true width back.
                app.set_grid_frozen_width(0.0);
            }
        }
        self.tip_tick(now);
        self.term_tick();
        self.surf7_tick(now);
        // The agent: its deadlines, and every resuming tab's reveal clock (a key, an exit or a
        // failed spawn reveal through the handle itself).
        self.hub.tick();
        if self.hub.take_changed() {
            self.sync_model_agents();
            changed = true;
        }
        let resuming: Vec<_> = self
            .deps
            .host
            .live_ids()
            .iter()
            .filter_map(|id| self.deps.host.handle(id))
            .filter(|h| h.resuming())
            .collect();
        for h in &resuming {
            h.reveal_tick();
        }
        if resuming.iter().any(|h| !h.resuming()) {
            changed = true;
        }
        if matches!(self.skel, Skel::Leaving(t) if now >= t) {
            changed = true;
        }
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
        let front = self.model.active_shell().map(|t| t.id.as_str());
        KeyContext {
            help: flare_settings::help_prefs::help_enabled(&self.prefs),
            vertical: self.vertical(),
            // A full-screen program owns Ctrl+F: the alternate screen or the mouse (PT focusedTermFullScreen).
            full_screen: self.front_handle().is_some_and(|h| {
                let m = h.mode();
                m.alt_screen || m.mouse != MouseTracking::None
            }),
            agent_keys: agent_keys(&self.prefs),
            agent_guard: agent_guard(&self.prefs),
            agent_here: front.is_some_and(|f| self.hub.agent_here(f)),
            agent_working: front.is_some_and(|f| self.hub.working(f)),
            // A selection the program made itself, or Flare's own selection in the grid: Ctrl+C
            // copies it (rule 50), the guard leaves the key alone.
            agent_selecting: self.term.program_selecting()
                || self
                    .front_tab()
                    .is_some_and(|f| self.term.has_selection(&f.info())),
        }
    }

    /// Order: the chord table first; then a Slint control with the keys, else the front shell.
    pub fn on_key(&mut self, ev: &KeyEvent) -> KeyOutcome {
        // Dictation sees every key first (Prism Terminal's capture phase); a swallowed key goes nowhere else.
        if self
            .dictation
            .feed_key(ev, false, &|| self.dictation_front())
        {
            return KeyOutcome::Consumed;
        }
        if ev.pressed {
            flare_term::session::trace_key("key");
            self.term.hide_thumb();
        }
        // A right-click menu takes no keyboard (PT ContextMenu.tsx): Escape closes it and never
        // reaches the shell; every other key goes where it would without the menu.
        if ev.pressed && ev.key == "Escape" && self.menu.is_some() {
            self.close_menu();
            return KeyOutcome::Consumed;
        }
        let mut ctx = self.key_context();
        // A text field of the window (the find bar, the sidebar find box, the help search) has the
        // keys: Ctrl+C and Ctrl+D there are the field's, never the agent guard's.
        if self
            .app()
            .is_some_and(|a| a.get_chrome_has_focus() || a.get_find_input_focused())
        {
            ctx.agent_guard = false;
        }
        let d = chords::dispatch(ev, &ctx, &mut Target(self));
        let shell_in_front = self.front_handle().is_some();
        // Start-up trace: the first key that reaches a tab already showing a prompt row is the
        // usable mark, and the one place the phase line is logged.
        if shell_in_front
            && crate::startup_trace::is_marked(crate::startup_trace::Phase::FirstPrompt)
            && !crate::startup_trace::is_marked(crate::startup_trace::Phase::Usable)
            && self.front_shows_prompt()
        {
            crate::startup_trace::mark(crate::startup_trace::Phase::Usable);
            log::info!("{}", crate::startup_trace::line());
        }
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
                // The Settings page has no shell, and its own fields (the find field, a colour code,
                // the popover's Escape) are Slint's: a key that is no chord goes to Slint there.
                if self.view.front_kind == 1
                    || self.app().is_some_and(|a| {
                        a.get_chrome_has_focus()
                            || a.get_find_input_focused()
                            || a.get_panel_has_focus()
                    })
                {
                    KeyOutcome::Slint
                } else if shell_in_front {
                    self.term_key(ev, &ctx)
                } else {
                    KeyOutcome::Consumed
                }
            }
        }
    }

    pub fn on_key_up(&mut self, ev: &KeyEvent) {
        if self
            .dictation
            .feed_key(ev, true, &|| self.dictation_front())
        {
            return;
        }
        chords::on_key_up(ev, &mut Target(self));
    }

    /// The window lost the keyboard: a Most recent walk ends, tips go, a right-click menu shuts
    /// (PT ContextMenu.tsx closes on the window's blur).
    pub fn on_focus_lost(&mut self) {
        // The keyboard moving into the page's child window sends this window a kill-focus too, but the
        // window is still the one in use: nothing to let go of. A real switch to another app can look the
        // same until Windows has finished it, so that case asks again one turn later.
        match crate::web_ctl::focus_loss(
            self.web.has_backend(),
            self.web.has_focus(),
            self.window_is_foreground(),
        ) {
            crate::web_ctl::FocusLoss::Skip => return,
            crate::web_ctl::FocusLoss::Check => {
                (self.deps.post)(UiMsg::FocusLostCheck);
                return;
            }
            crate::web_ctl::FocusLoss::Now => {}
        }
        self.blur();
    }

    /// The window is no longer the one in use: dictation, the walk, tips and menus let go.
    fn blur(&mut self) {
        crate::dictation_ctl::on_blur();
        self.model.commit_walk();
        self.clear_tip();
        self.term.hide_thumb();
        self.close_menu();
        self.on_focus(false);
    }

    // ---- pointer and files ------------------------------------------------------------------------------------------

    /// A tab was picked (strip, sidebar row or overflow list): the front tab is read at once, so a
    /// Finished/Failed mark held since the window was away comes down if the pick CHANGED the tab in
    /// front (the tab left loses it), and stays if it was the tab already in front (owner, 2026-10-23).
    fn tab_picked(&mut self) {
        self.sync_looking();
        if self.hub.take_changed() {
            self.sync_model_agents();
        }
    }

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
        // A press anywhere ends a key capture on the Dictation page (the field's own click follows it).
        crate::dictation_ctl::on_press();
        // A press on Flare's own window (the page's window takes its own) is a press away from the page.
        if self.web.has_focus()
            && let Some(app) = self.app()
        {
            let hwnd = crate::window::hwnd_of(&app);
            self.web.release_focus(hwnd);
        }
        self.clear_tip();
        self.term.hide_thumb();
        if self.flight.on() {
            self.flight.pointer_down();
            self.refresh();
        }
    }

    /// Whether a file drag hovers the window (the strip is off window-drag meanwhile).
    pub fn drag_in_flight(&self) -> bool {
        self.flight.on()
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
            // Below the title row and right of the sidebar: the terminal's (a typed, quoted path), unless the pointer
            // is over the side panel: a file dropped there is not meant for the shell on the left.
            let over_panel = self.app().is_some_and(|a| {
                a.get_panel_shown() && cursor_css.0 >= a.get_grid_left() + a.get_panel_slot_x()
            });
            if cursor_css.1 >= BAR_H && !over_panel {
                let front = self.front_tab();
                {
                    let info = front.as_ref().map(FrontTab::info);
                    self.term.on_dropped(info.as_ref(), &path);
                }
                self.wake_term();
            }
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
        self.web.parent_moved();
        self.maximised = o.maximised;
        self.on_minimised(o.minimised);
        if let Some(state) = self.bounds.state(o.maximised) {
            self.deps.window_store.save(&state);
        }
    }

    /// The grid area changed (Task 30b's `grid-resized`, after the grid host resized): tabs born
    /// from now on start at this size and the front tab follows.
    pub fn on_grid_dims(&mut self, cols: usize, rows: usize) {
        self.grid_fits += 1;
        self.grid_dims = (cols, rows);
        let front = self.front_id();
        self.deps.host.set_dims(cols, rows, front.as_deref());
    }

    /// The window hands over the Settings controller (Task 41).
    pub fn set_settings(&mut self, s: Rc<RefCell<SettingsCtl>>) {
        self.settings = Some(s);
        self.settings_shown = false;
        self.settings_present = false;
        self.sync_settings();
    }

    /// Tells the Settings controller when its page comes to the front, goes behind another tab,
    /// or its tab is closed (the page then resets to Appearance).
    fn sync_settings(&mut self) {
        let Some(s) = self.settings.clone() else {
            return;
        };
        let shown = self.view.front_kind == 1;
        let present = self
            .model
            .state
            .tabs
            .iter()
            .any(|t| t.kind == TabKind::Settings);
        let (was_shown, was_present) = (self.settings_shown, self.settings_present);
        self.settings_shown = shown;
        self.settings_present = present;
        let Ok(mut s) = s.try_borrow_mut() else {
            return;
        };
        if was_present && !present {
            s.on_settings_tab(false);
        }
        if shown && !was_shown {
            s.show();
        } else if !shown && was_shown {
            s.hide();
        }
    }

    /// The window hands over what stops the hand-off server at the end of life.
    pub fn set_handoff_stop(&mut self, stop: Rc<dyn Fn()>) {
        self.handoff_stop = Some(stop);
    }

    /// The window hands over the font controller; the face and size in force apply at once.
    pub fn set_font_ctl(&mut self, f: FontCtl) {
        self.font = Some(f);
        self.apply_font();
    }

    /// The face and size in force for the tab in front (its own zoom, else the settings). A change of
    /// the cell grid refits every open tab, visible or not, through the resize path (Task 27).
    pub fn apply_font(&mut self) {
        let front = self.model.active_shell().map(|t| t.id.clone());
        let ov = front.and_then(|id| self.deps.host.font_override(&id));
        let Some(f) = self.font.as_mut() else { return };
        if let Some((cols, rows)) = f.apply(ov)
            && cols >= 2
            && rows >= 1
        {
            self.grid_dims = (cols, rows);
            let front = self.front_id();
            if ov.is_some() {
                // The front tab wears its own zoom: only it takes this grid.
                self.deps.host.set_dims(cols, rows, front.as_deref());
            } else {
                // The settings' size: every tab without a zoom of its own follows.
                self.deps.host.refit_unzoomed(cols, rows);
            }
        }
        if let Some(app) = self.app() {
            app.window().request_redraw();
        }
    }

    /// Ctrl+wheel over the grid: the tab in front zooms one pixel per notch (never saved). False when
    /// there is no shell in front to zoom.
    pub fn zoom_front(&mut self, delta_y: f64) -> bool {
        let Some(id) = self.model.active_shell().map(|t| t.id.clone()) else {
            return false;
        };
        let Some(f) = self.font.as_mut() else {
            return false;
        };
        if delta_y == 0.0 || !delta_y.is_finite() {
            return false;
        }
        let ov = self.deps.host.font_override(&id);
        let next = zoom_step(
            effective_px(ov, f.pct()),
            if delta_y > 0.0 { 1 } else { -1 },
        );
        self.deps.host.set_font_override(&id, Some(next));
        // The front tab only: the others keep their size until they come to the front.
        if let Some((cols, rows)) = f.apply(Some(next))
            && cols >= 2
            && rows >= 1
        {
            self.on_grid_dims(cols, rows);
        }
        if let Some(app) = self.app() {
            app.window().request_redraw();
        }
        true
    }

    /// Folders from a second launch: raise the window, then each folder is a new tab.
    pub fn handoff(&mut self, h: Handoff) {
        (self.deps.raise)();
        // `--preview-update` on a second launch shows the update window in the running app; with a
        // real offer present the engine changes nothing. Alone, it opens no tab.
        if flare_update::preview::wants_preview(&h.args) {
            (self.deps.post)(UiMsg::Update(UpdateMsg::PreviewRequested));
            // The program's own name may head the list; only flags besides it means nothing to open.
            if (h.args.iter()).all(|a| a.starts_with("--") || a.to_lowercase().ends_with(".exe")) {
                return;
            }
        }
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

    /// The agents at work now: the tab's label, the agent, and for how long.
    fn busy_agents(&self) -> Vec<(String, DetectedAgent, u64)> {
        let labels = self.model.labels();
        self.model
            .state
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TabKind::Shell && self.agent_present(&t.id))
            .filter_map(|(i, t)| {
                let for_ms = self.working_for_ms(&t.id)?;
                Some((
                    labels.get(i).cloned().unwrap_or_default(),
                    self.agent_named(&t.id),
                    for_ms,
                ))
            })
            .collect()
    }

    /// Every route out of the window ends here. A working agent vetoes the close and asks.
    pub fn on_close_requested(&mut self) -> CloseDecision {
        if self.update_agreed.load(Ordering::SeqCst) {
            self.close_flow.agree();
        }
        let busy = self.busy_agents();
        self.close_flow.set_busy(holds_window_close(busy.len()));
        match self.close_flow.on_close() {
            CloseVerdict::Ask => {
                self.drop_install_ask();
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

    /// The `tabs-saved` crumb of the last save: per shell tab, the agent `tabs.json` keeps for it and what the
    /// hub knew, so a restart that resumes nothing can be told apart from a save that kept no agent.
    fn note_saved_agents(&self) {
        let tabs: Vec<serde_json::Value> = self
            .model
            .state
            .tabs
            .iter()
            .filter(|t| t.kind == TabKind::Shell)
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "saved": self.model.agent_kind(&t.id).map(|k| format!("{k:?}").to_lowercase()),
                    "here": self.hub.agent_here(&t.id),
                    "kind": self.hub.agent_kind(&t.id).map(|k| format!("{k:?}").to_lowercase()),
                    "session": self.hub.claude_session(&t.id).is_some(),
                })
            })
            .collect();
        crate::diag_ctl::crumb("tabs-saved", &[("tabs", serde_json::Value::Array(tabs))]);
    }

    /// The end of life: both state files flushed, then every shell killed and waited for.
    fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.staged_on_finish();
        // Stop listening before anything slow: a launch that connects while the shells are killed
        // and waited for would get its folders acknowledged and then dropped.
        if let Some(stop) = self.handoff_stop.take() {
            stop();
        }
        // Dictation first (parity 834): the engine's children, the link and the key hook stop before the shells are
        // killed and waited for, so a picker never keeps swallowing digits while the window is already hidden.
        crate::dictation_ctl::stop_all();
        self.save_window_now();
        if let Some(s) = self.model.saved_if_restored() {
            self.deps.tabs_store.save(&s);
            self.note_saved_agents();
        }
        if self.model.restore_done() {
            let tabs = self.shell_tabs();
            self.panels.persist(&tabs);
        }
        self.panels.flush();
        self.prefs.flush();
        // The browser goes before the window does (rule 28): its views are closed and its calls answered
        // while the window and the COM apartment are still up, so no callback lands during the teardown.
        self.mods_shutdown();
        self.web.close_all();
        self.downloads.drain();
        self.app_asks.drain();
        (self.deps.hide)();
        end_of_life(
            &self.deps.tabs_store,
            &self.deps.window_store,
            &mut self.deps.host,
        );
        // The shells are gone: the `quit` line is the last line of the log.
        crate::diag_ctl::quit();
    }

    // ---- the close question ------------------------------------------------------------------------------------------------

    fn show_ask(&mut self) {
        self.clear_tip();
        self.term.hide_thumb();
        self.close_menu_quietly();
        self.close_overflow_quietly();
        // A question is asked over nothing: the Command help popup is put away first, so it is never
        // underneath it and its field does not hold the keys the question needs.
        if self.ask.is_some()
            && let Some(h) = self.help.clone()
            && let Ok(mut h) = h.try_borrow_mut()
        {
            h.close();
        }
        // The update window steps aside for any close question, as an install's does (it is
        // already borrowed while an install asks, and that path hides it itself).
        if self.ask.is_some()
            && let Some(u) = self.update.clone()
            && let Ok(mut u) = u.try_borrow_mut()
            && u.window_up()
        {
            u.hide();
            self.update_stepped_aside = true;
        }
        if let Some(app) = self.app() {
            // A question has the keys: a page that holds them (Ctrl+W pressed in it) gives them back.
            if self.ask.is_some() {
                self.web.release_focus(crate::window::hwnd_of(&app));
            }
            match &self.ask {
                Some(a) => {
                    let v = ask_view(a, &crate::chrome_apply::text_ink());
                    let body = slint::StyledText::from_markdown(&v.body)
                        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&v.body));
                    app.set_dialog_title(v.title.into());
                    app.set_dialog_body(body);
                    app.set_dialog_primary(v.primary.into());
                    // PT's close question never uses the danger fill: the confirm is the solid accent.
                    app.set_dialog_danger(false);
                    app.set_dialog_default_cancel(false);
                    app.set_dialog_by_key(std::mem::take(&mut self.ask_by_key));
                    // An app link's question waiting under it shows again later, with its guard from then.
                    self.app_asks.unshown();
                    app.set_dialog_open(true);
                }
                None => app.set_dialog_open(false),
            }
        }
        self.refresh();
    }

    fn dialog_cancel(&mut self) {
        if self.ask.is_none() && self.downloads.current().is_some() {
            self.answer_download(false);
            return;
        }
        if self.ask.is_none() && self.app_asks.current().is_some() {
            self.answer_app(false);
            return;
        }
        self.ask = None;
        let install = self.install_cb.take();
        self.show_ask();
        let stepped = std::mem::take(&mut self.update_stepped_aside);
        // A download waiting behind the question is asked now (and an app link after the downloads).
        self.show_download_ask();
        match install {
            // The update window comes back and lands its own focus.
            Some(cb) => cb(false),
            None if stepped => self.update_show_again(),
            None => self.focus_terminal(),
        }
    }

    /// The update window that stepped aside for a close question comes back (the question was cancelled).
    fn update_show_again(&mut self) {
        if let Some(u) = self.update.clone()
            && let Ok(mut u) = u.try_borrow_mut()
        {
            u.show_again();
        }
    }

    fn dialog_confirm(&mut self) {
        if self.ask.is_none() && self.downloads.current().is_some() {
            self.answer_download(true);
            return;
        }
        if self.ask.is_none() && self.app_asks.current().is_some() {
            self.answer_app(true);
            return;
        }
        let Some(ask) = self.ask.take() else { return };
        // Confirmed: the window stays put away (the tab is closing, or the install is going ahead).
        self.update_stepped_aside = false;
        let install = self.install_cb.take();
        self.show_ask();
        if ask.install {
            // The go-ahead for the install only: the app quits later, when the installer starts.
            if let Some(cb) = install {
                cb(true);
            }
            return;
        }
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
            UiMsg::StarterFolders(list) => self.starters = list,
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
            UiMsg::LookChanged => self.on_look_changed(),
            UiMsg::PrefsChanged => self.on_prefs_changed(),
            UiMsg::Settings(m) => {
                if let Some(s) = self.settings.clone()
                    && let Ok(mut s) = s.try_borrow_mut()
                {
                    s.on_msg(m);
                }
            }
            UiMsg::Update(m) => {
                if let Some(u) = self.update.clone()
                    && let Ok(mut u) = u.try_borrow_mut()
                {
                    u.on_msg(m);
                }
            }
            UiMsg::Help => {
                self.help_dirty.store(false, Ordering::SeqCst);
                if let Some(h) = self.help.clone()
                    && let Ok(mut h) = h.try_borrow_mut()
                {
                    h.on_pref_change();
                }
            }
            UiMsg::Panel(PanelMsgBox(m)) => {
                // A markdown page that arrived (or failed) changes what the browser shows: the whole window follows.
                if let Some(app) = self.app()
                    && self.panels.on_msg(&app, m)
                {
                    self.refresh();
                }
            }
            UiMsg::FocusLostCheck => {
                if !self.window_is_foreground() && !self.web.has_focus() {
                    self.blur();
                }
            }
            UiMsg::Convo(ConvoMsgBox(m)) => self.on_convo_msg(m),
            UiMsg::AppLinkFailed { tab, scheme, error } => {
                self.app_link_failed(&tab, &scheme, &error);
            }
            UiMsg::DevLook { tab, token, look } => self.on_dev_look(&tab, token, look),
            UiMsg::DevClickLook { tab, token, look } => self.on_dev_click_look(&tab, token, look),
            UiMsg::ToastClicked(tab) => self.on_toast_clicked(&tab),
            UiMsg::ToastFailed => self.on_toast_failed(),
            UiMsg::Mods(crate::mods_ctl::ModsMsgBox(m)) => self.on_mods_msg(m),
            UiMsg::Staged(m) => self.on_staged_msg(m),
            UiMsg::Web(WebMsgBox(m)) => {
                if let Some(app) = self.app() {
                    // A DevTools answer or a timer turn changes the window's properties only when a
                    // call starts or ends or a hold ends (a look of a big page is hundreds of answers).
                    let quiet = matches!(
                        &m,
                        crate::web_ctl::WebMsg::AgentTick
                            | crate::web_ctl::WebMsg::Event(flare_web::WebEvent::Cdp { .. })
                    );
                    let before = self.web.agent_shape();
                    let outs = self.web.on_msg(&app, m);
                    self.apply_web(outs);
                    if !quiet || self.web.agent_shape() != before {
                        self.web_moved();
                    }
                }
            }
            UiMsg::Ctl(CtlRequestBox(req)) => {
                // The caller stopped waiting at its deadline and was told `timeout`: a request that
                // arrives later changes nothing (and the late answer has no reader).
                if std::time::Instant::now() > req.deadline {
                    // A mini-app asked for too late is never shown: its folder goes.
                    if let CtlOp::ShowApp { spec, .. } = req.op {
                        self.panels.remove_folder(spec);
                    }
                    let _ = req.reply.send(Err(flare_ctl::CtlError::new(
                        flare_ctl::code::TIMEOUT,
                        "Flare's window did not answer in time",
                    )));
                } else {
                    match req.op {
                        // These wait for the page to load: the browser's message answers them.
                        op @ (CtlOp::BrowserOpen { .. } | CtlOp::BrowserNavigate { .. }) => {
                            self.ctl_browser(op, req.reply, req.deadline);
                        }
                        // The agent uses the page: the browser's messages answer these too.
                        op @ CtlOp::BrowserAgent { .. } => {
                            self.ctl_browser_agent(op, req.reply);
                        }
                        // A mini-app or a diagram: answered when the page has loaded (and drawn).
                        op @ CtlOp::ShowApp { .. } => {
                            self.ctl_show_app(op, req.reply, req.deadline);
                        }
                        // A mod's panel, or a tool call put in front of a mod's page (stage 8).
                        op @ CtlOp::ShowMod { .. } => self.ctl_show_mod(op, req.reply),
                        op @ CtlOp::ModTool { .. } => self.ctl_mod_tool(op, req.reply),
                        // A plugin's question (the `fill` permission): answered by the person, or when it expires.
                        op @ (CtlOp::Fill { .. } | CtlOp::SecretAsk { .. }) => {
                            self.ctl_plugin_ask(op, req.reply);
                        }
                        op => {
                            let answer = self.ctl_op(op);
                            let _ = req.reply.send(answer);
                        }
                    }
                }
            }
            UiMsg::Term(TermMsgBox(TermMsg::Preview {
                tab,
                kind,
                abs,
                asked,
            })) => {
                if asked {
                    self.show_preview_asked(&tab, kind, &abs);
                } else {
                    self.show_preview(&tab, kind, &abs);
                }
            }
            UiMsg::Term(TermMsgBox(TermMsg::PreviewRefused { tab })) => {
                // The person picked "Show in side panel" and the file is no longer showable: say so. A tab with no
                // panel open gets the words as a note in one; a tab whose panel is open keeps what it shows (a mini-app
                // or a page would be ended by a note) and says it in the panel's header or the page's bar.
                const WHY: &str = "This file cannot be shown here. It may have changed or been removed since the menu opened.";
                const SHORT: &str = "Cannot show that file: it changed or was removed";
                if !self.is_shell_tab(&tab) {
                    return;
                }
                if self.panels.is_open(&tab) {
                    if let Some(app) = self.app() {
                        let page = (self.panels.web_target(&tab).is_some()
                            && !self.panels.is_app(&tab))
                            || self.panels.is_md_page(&tab);
                        if page {
                            self.web.notice(&tab, SHORT);
                        } else {
                            self.panels.refuse(&app, &tab, SHORT);
                        }
                    }
                    self.refresh();
                } else if self
                    .panels
                    .show_text(&tab, WHY, Some("Cannot show this file"))
                {
                    self.refresh();
                    self.panel_changed(&tab);
                }
            }
            UiMsg::Term(TermMsgBox(m)) => {
                let front = self.front_tab();
                {
                    let info = front.as_ref().map(FrontTab::info);
                    self.term.on_msg(info.as_ref(), m);
                }
                self.push_term();
                self.wake_term();
            }
        }
    }

    /// The waker: drain what the shells reported. A folder report that is a share or holds a
    /// control character is never adopted (a program must not steer a tab to SMB), and a shell
    /// that exited closes its tab unasked.
    /// Each live tab's scrollback bytes (the memory breakdown).
    pub fn scrollbacks(&self) -> Vec<(String, usize)> {
        self.deps.host.scrollbacks()
    }

    pub fn host_wake(&mut self) {
        flare_term::session::trace_key("wake");
        let events = self.deps.host.drain_events();
        let mut changed = false;
        let mut terminal_news = false;
        self.sync_hub();
        self.hub.on_host_events(&events);
        self.hub.drain_poll();
        if self.hub.take_changed() {
            self.sync_model_agents();
            self.close_flow
                .set_busy(holds_window_close(self.hub.working_count()));
            changed = true;
        }
        if self.clear_agent_decor() {
            changed = true;
        }
        // A hook may have named a new Claude session: a tracker follows it (only for an open Changes panel).
        if self.sync_convo() {
            changed = true;
        }
        for ev in events {
            self.shell_crumb(&ev);
            match ev {
                HostEvent::Revealed { .. } => changed = true,
                HostEvent::Cwd { id, path } => {
                    if !is_unsafe_restore_folder(&path) && self.model.set_cwd(&id, &path) {
                        changed = true;
                    }
                }
                HostEvent::Output { id } => self.dev_read(&id),
                HostEvent::Exited { id, .. } => {
                    self.note_close(&id);
                    self.deps.host.kill(&id);
                    self.working.remove(&id);
                    self.term.tab_closed(&id);
                    self.forget_tab_surfaces(&id);
                    if self.model.close(&id) {
                        changed = true;
                    }
                }
                HostEvent::ClipboardWrite { id, text } => {
                    let front = self.front_id();
                    self.term.on_term_event(
                        &id,
                        front.as_deref(),
                        &TermEvent::ClipboardWrite(text),
                    );
                    terminal_news = true;
                }
                // The handle lets one bell through a second per tab: the button flashes, no sound.
                HostEvent::Bell { .. } => (self.flash)(),
                _ => {}
            }
        }
        // Output on the tab in front: the find refreshes after the last of it, and the drawn frame
        // may ask which printed paths exist (a few timer rounds pump those questions).
        if let Some(front) = self.front_id() {
            self.term.on_output(&front);
            self.term_hot = 2;
            terminal_news = true;
        }
        if changed {
            self.refresh();
        } else if let Some(app) = self.app() {
            app.window().request_redraw();
        }
        if terminal_news {
            self.push_term();
            self.wake_term();
        }
    }
}

/// The chord actions, acting on the controller.
struct Target<'a>(&'a mut AppCtl);

impl ChordTarget for Target<'_> {
    fn blocked(&self) -> bool {
        self.0.ask.is_some() || self.0.update_up()
    }

    fn toggle_fullscreen(&mut self) {
        // A page in full screen: F11 takes it out, and the window goes back to what it was before the page.
        if self.0.web.covers_window() {
            self.0.web.leave_full();
            self.0.web_moved();
            return;
        }
        if let Some(app) = self.0.app() {
            // The UI turns solid and the backdrop comes off before the OS is asked (D7).
            crate::look_ctl::LookCtl::with_current(|l| l.before_fullscreen());
            let w = app.window();
            w.set_fullscreen(!w.is_fullscreen());
        }
    }

    fn toggle_help(&mut self) {
        self.0.help_toggle();
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
            // A chord opened it: the focus ring shows from the start, as :focus-visible does after a key.
            self.0.ask_by_key = true;
            self.0.request_close(&id);
            self.0.ask_by_key = false;
        }
    }

    fn find(&mut self) {
        self.0.toggle_find();
    }

    fn open_system_menu(&mut self) {
        self.0.open_system_menu();
    }

    fn toggle_panel(&mut self) {
        self.0.toggle_front_panel();
    }

    fn focus_panel(&mut self) {
        self.0.move_panel_focus();
    }
}

/// How many times the system menu was asked for (the `--e2e` hook reads it; a pointer event
/// never raises it).
static SYSTEM_MENU_REQUESTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The system menu requests so far.
pub fn system_menu_requests() -> u32 {
    SYSTEM_MENU_REQUESTS.load(Ordering::SeqCst)
}

impl AppCtl {
    /// The page the Settings tab holds (kept while its tab stays open); `None` before the Settings
    /// controller exists or while it is borrowed. The `settings.state` probe reads it (Task 21).
    pub fn settings_page(&self) -> Option<&'static str> {
        self.settings
            .as_ref()
            .and_then(|s| s.try_borrow().ok().map(|c| c.current_page()))
    }

    /// Alt+Space: counts the request; outside `--e2e` the system menu shows at the window's
    /// top-left (the e2e never opens a native menu: it would take the foreground).
    pub fn open_system_menu(&mut self) {
        SYSTEM_MENU_REQUESTS.fetch_add(1, Ordering::SeqCst);
        if self.deps.e2e {
            return;
        }
        if let Some(hwnd) = self.app().and_then(|a| crate::window::hwnd_of(&a)) {
            // The menu runs its own message loop: shown from a zero timer, once this key is handled
            // and the controller is let go, so shell output and timers reach it while the menu is up.
            slint::Timer::single_shot(Duration::ZERO, move || {
                flare_win::sysmenu::show_at_top_left(hwnd)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    mod ask_tests;
    mod md_tests;
    mod mods_tests;
    mod splitter_tests;
    mod web_tests;
    use super::*;
    use crate::menus_ui::CHOOSE_FOLDER;
    use crate::openers::{OpenedRecord, RecordingOpeners};
    use crate::ui_harness::{Frame, Harness};
    use flare_render::glyph::{FakeGlyphs, GlyphSource};
    use flare_render::metrics::cell_metrics;
    use flare_settings::new_tab::set_new_tab_folder;
    use flare_settings::tab_layout::set_tab_layout;
    use flare_settings::term_prefs::set_agent_keys;
    use flare_shells::list::ShellDef;
    use flare_tabs::restore::{NoSessions, SavedTab, tabs_json};
    use flare_term::fake::{FakeShells, FakeSpawner};
    use flare_win::clipboard_port::MemoryClipboard;
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
        shells: Option<Vec<ShellDef>>,
        saved: Vec<(&'static str, Option<AgentKind>)>,
        active: usize,
        launch: Vec<String>,
        real_fs: bool,
        pick_answer: Option<PathBuf>,
        /// The window's display scale; 0 is 1.
        scale: f32,
        /// The restore's Claude session lookup (None: no sessions).
        lookup: Option<Arc<dyn SessionLookup>>,
        /// The plugin folder the host hands new shells (None: the environment is untouched).
        plugin_dir: Option<String>,
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
        clip: Arc<MemoryClipboard>,
        openers: Arc<RecordingOpeners>,
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
        let shells = match opts.shells.clone() {
            Some(defs) => FakeShells::with(defs),
            None => FakeShells::new(),
        };
        let host = TabHost::new(
            sp.clone(),
            shells,
            prefs.clone(),
            waker,
            opts.plugin_dir.clone(),
        );
        let clip = Arc::new(MemoryClipboard::new());
        let openers = Arc::new(RecordingOpeners::new());
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
            lookup: opts.lookup.clone().unwrap_or_else(|| Arc::new(NoSessions)),
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
            starter_folders: Rc::new(|_| {}),
            launch_folders: opts.launch,
            e2e: true,
            term: TermPorts {
                clip_port: clip.clone(),
                openers: openers.clone(),
                agent: None,
                dbl: flare_win::dblclick::FALLBACK,
                stat: Arc::new(flare_term::path_open::FsStat),
                metrics: Rc::new(|| cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0)),
                mark_grid_dirty: Rc::new(|| {}),
            },
        };
        let scale = if opts.scale > 0.0 { opts.scale } else { 1.0 };
        let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
        let ctl = AppCtl::new(&h.comp, deps);
        // Pinned: the runner's Windows "show animations" switch must not decide the clock or the marks (the help panel and chrome motion still read the OS).
        ctl.borrow_mut().set_animations_source(Rc::new(|| true));
        Fx {
            h,
            ctl,
            q,
            sp,
            dir,
            clock,
            log,
            prefs,
            clip,
            openers,
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

        /// The fake process of tab `id`: the one whose environment carries that tab's token. `None` until
        /// the spawn has registered the session.
        fn shell_of_tab(&self, id: &str) -> Option<Arc<flare_term::fake::FakeState>> {
            let token = self.ctl.borrow().deps.host.manager().tab_token(id)?;
            self.sp
                .made()
                .into_iter()
                .find(|m| env_of(m, flare_shells::env::TAB_ID_VAR).as_deref() == Some(&token))
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
        let raw = std::fs::read_to_string(f.dir.path().join("tabs.json")).ok()?; // sync-ok: test code only, never the app
        Some(flare_tabs::restore::parse_tabs(&raw).tabs.len())
    }

    #[test]
    fn ctrl_wheel_zooms_the_front_tab_only_and_a_switch_wears_each_tabs_own_size() {
        use crate::host::{GridHost, GridPainter};
        use flare_render::glyph::FakeGlyphs;
        use flare_settings::look::LookStore;
        let f = fx();
        f.start_restore_and_wait();
        let grid = Rc::new(RefCell::new(GridHost::new(
            Box::new(FakeGlyphs::new(8, 16)),
            GridPainter::Cpu,
        )));
        grid.borrow_mut().resize(648, 264, 1.0);
        f.ctl
            .borrow_mut()
            .set_font_ctl(FontCtl::new(LookStore::new(f.prefs.clone()), grid.clone()));
        let base = f.ctl.borrow().grid_dims;
        f.ctl.borrow_mut().new_tab();
        f.ctl.borrow_mut().new_tab();
        let b = f.ctl.borrow().front_id().unwrap();
        assert!(f.ctl.borrow_mut().zoom_front(1.0));
        assert!(f.ctl.borrow_mut().zoom_front(1.0));
        assert_eq!(f.ctl.borrow().deps.host.font_override(&b), Some(15));
        let zoomed = f.ctl.borrow().grid_dims;
        assert_ne!(zoomed, base, "the grid follows the zoom");
        // Switching to the other tab wears the settings' size again; back to b wears its zoom.
        let rows = f.ctl.borrow().model.state.tabs.len();
        assert!(rows >= 2);
        let a = f.ctl.borrow().model.state.tabs[rows - 2].id.clone();
        f.ctl.borrow_mut().model.pick(&a);
        f.ctl.borrow_mut().refresh();
        assert_eq!(f.ctl.borrow().grid_dims, base);
        f.ctl.borrow_mut().model.pick(&b);
        f.ctl.borrow_mut().refresh();
        assert_eq!(f.ctl.borrow().grid_dims, zoomed);
        // Nothing of it is stored.
        assert_eq!(f.prefs.get("flare.term.fontPct"), None);
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
    fn a_stored_ask_mode_opens_in_the_folder_at_once() {
        // There is no ask mode any more (2026-10-27): a profile that still says "ask" opens like folder mode.
        let f = fx_with_prefs(Opts::default(), |p| {
            p.set("flare.newtab.mode", "ask");
            set_new_tab_folder(p, "D:\\stored");
        });
        f.ctl.borrow_mut().new_tab();
        f.wait_made("D:\\stored");
        assert_eq!(f.cwds(), ["D:\\stored"]);
        assert!(f.log.borrow().picks.is_empty(), "no picker");
    }

    #[test]
    fn choose_a_folder_in_the_folder_menu_asks_opens_there_and_remembers_it() {
        let f = fx_with_prefs(
            Opts {
                pick_answer: Some(PathBuf::from("D:\\w")),
                ..Opts::default()
            },
            |p| set_new_tab_folder(p, "D:\\stored"),
        );
        f.ctl.borrow_mut().open_folder_menu(212.0, 400.0, 434.0);
        assert!(f.h.comp.get_menu_open());
        assert!(
            f.h.comp.get_menu_drop(),
            "the arrow's menu is a dropdown from the button"
        );
        assert_eq!(
            (
                f.h.comp.get_menu_x(),
                f.h.comp.get_menu_y(),
                f.h.comp.get_menu_foot()
            ),
            (212.0, 400.0, 434.0)
        );
        let first = f.h.comp.get_menu_rows().row_data(0).unwrap();
        assert_eq!(first.label.as_str(), CHOOSE_FOLDER);
        f.h.comp.invoke_menu_picked(0);
        assert!(!f.h.comp.get_menu_open());
        // The picker starts in the new-tab folder; its answer opens a tab and joins the recents.
        assert_eq!(
            f.log.borrow().picks,
            vec![Some(PathBuf::from("D:\\stored"))]
        );
        f.pump_until("the tab", |f| f.rows() == 1);
        assert_eq!(f.cwds(), ["D:\\w"]);
        f.wait_made("D:\\w");
        assert_eq!(
            flare_settings::recent_roots::recent_roots(&f.prefs)
                .first()
                .map(String::as_str),
            Some("D:\\w")
        );
        // The strip's right-click menu opens at the pointer, not as a dropdown.
        f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        assert!(!f.h.comp.get_menu_drop());

        // A cancel opens nothing.
        let g = fx();
        g.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        g.h.comp.invoke_menu_picked(0);
        g.pump();
        assert_eq!(g.log.borrow().picks, vec![Some(PathBuf::from("C:\\home"))]);
        assert_eq!(g.rows(), 0);
    }

    #[test]
    fn the_starter_folders_fill_the_folder_menu_once_the_worker_answers() {
        let f = fx();
        f.ctl.borrow_mut().on_msg(UiMsg::StarterFolders(vec![
            "C:\\home\\Documents".to_string(),
            "C:\\home".to_string(),
        ]));
        f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        let rows = f.h.comp.get_menu_rows();
        let labels: Vec<String> = (0..rows.row_count())
            .map(|i| rows.row_data(i).unwrap().label.to_string())
            .collect();
        assert_eq!(labels, [CHOOSE_FOLDER, "Documents", "home"]);
        assert_eq!(rows.row_data(2).unwrap().hint.as_str(), "user folder");
        f.h.comp.invoke_menu_picked(1);
        f.pump_until("the tab", |f| f.rows() == 1);
        assert_eq!(f.cwds(), ["C:\\home\\Documents"]);
    }

    #[test]
    fn a_folder_named_choose_a_folder_opens_itself_not_the_picker() {
        let named = format!("C:\\home\\{CHOOSE_FOLDER}");
        let f = fx();
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::StarterFolders(vec![named.clone()]));
        f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        let rows = f.h.comp.get_menu_rows();
        assert_eq!(rows.row_data(1).unwrap().label.as_str(), CHOOSE_FOLDER);
        f.h.comp.invoke_menu_picked(1);
        f.pump_until("the tab", |f| f.rows() == 1);
        assert_eq!(f.cwds(), [named.as_str()]);
        assert!(f.log.borrow().picks.is_empty(), "no picker");
    }

    #[test]
    fn a_dropped_folder_on_the_tab_row_opens_a_tab_and_a_file_opens_its_folder() {
        let f = fx_with(Opts {
            real_fs: true,
            ..Opts::default()
        });
        let folder = f.dir.path().join("proj");
        std::fs::create_dir(&folder).unwrap(); // sync-ok: test code only, never the app
        let file = folder.join("a.txt");
        std::fs::write(&file, "x").unwrap(); // sync-ok: test code only, never the app
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
    fn keyboard_focus_returns_to_the_terminal_on_the_frame_after_a_drag() {
        let f = fx();
        let a = f.ctl.borrow_mut().open_folder("C:\\a");
        let _ = f.ctl.borrow_mut().open_folder("C:\\b");
        f.wait_made("C:\\a");
        f.wait_made("C:\\b");
        // The find box holds the keys when the drag starts.
        f.h.comp.set_find_open(true);
        f.h.comp.invoke_focus_find();
        assert!(f.h.comp.get_find_input_focused());
        let x = {
            let c = f.ctl.borrow();
            let r = &c.view().rows[0];
            (f64::from(r.x) + f64::from(r.w) / 2.0) / c.scale
        };
        f.h.comp
            .invoke_strip_press(a.as_str().into(), 0, x as f32, 0.0);
        f.h.comp.invoke_strip_moved((x + 80.0) as f32);
        f.h.comp.invoke_strip_released();
        f.pump();
        assert!(
            !f.h.comp.get_find_input_focused(),
            "the keys went back to the terminal when the drag ended"
        );
    }

    #[test]
    fn restore_runs_once_even_if_asked_twice() {
        let lookup = FixedSessions::new(&[SESSION_A]);
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", Some(AgentKind::Claude))],
            active: 1,
            lookup: lookup_of(&lookup),
            ..Opts::default()
        });
        AppCtl::start_restore(&f.ctl);
        AppCtl::start_restore(&f.ctl);
        f.pump_until("the restore", |f| f.ctl.borrow().model().restore_done());
        f.pump_until("both spawns", |f| f.sp.made().len() >= 2);
        // A second run would answer after the first: give it the time to.
        std::thread::sleep(Duration::from_millis(100));
        f.pump();
        // The lookup is asked once, and each saved folder is spawned once.
        assert_eq!(lookup.asked(), ["C:\\b"], "the lookup runs once");
        assert_eq!(f.made_with("C:\\a").len(), 1, "a spawned once");
        assert_eq!(f.made_with("C:\\b").len(), 1, "b spawned once");
        assert_eq!(f.cwds(), ["C:\\a", "C:\\b"]);
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
    fn a_profile_stored_as_vertical_starts_vertical_on_the_first_frame() {
        let f = fx_with_prefs(
            Opts {
                saved: vec![("C:\\a", None), ("C:\\b", None)],
                ..Opts::default()
            },
            |p| set_tab_layout(p, TabLayout::Vertical),
        );
        assert!(f.h.comp.get_layout_vertical(), "built horizontal");
        assert!(f.h.comp.get_has_tabs(), "the placeholders show the sidebar");
        f.start_restore_and_wait();
        assert!(
            f.h.comp.get_layout_vertical(),
            "horizontal after the restore"
        );
        f.pump();
        f.ctl.borrow_mut().refresh();
        assert!(f.h.comp.get_layout_vertical());
    }

    #[test]
    fn a_vertical_profile_with_no_tabs_keeps_vertical_but_has_no_tabs_for_the_sidebar() {
        // `side-mode` in app.slint is `layout-vertical && has-tabs`, so the zero-tab start screen
        // shows the top bar by design until the owner decides otherwise (features.md, "Vertical
        // tabs at start"). This pins both inputs: the layout is vertical, there are no tabs.
        let f = fx_with_prefs(
            Opts {
                saved: vec![],
                ..Opts::default()
            },
            |p| set_tab_layout(p, TabLayout::Vertical),
        );
        f.start_restore_and_wait();
        f.pump();
        f.ctl.borrow_mut().refresh();
        assert!(f.h.comp.get_layout_vertical());
        assert!(!f.h.comp.get_has_tabs(), "a start screen has no tabs");
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
    fn a_zoom_never_resizes_the_other_shells_and_a_switch_keeps_each_zoomed_size() {
        use crate::host::{GridHost, GridPainter};
        use flare_render::glyph::FakeGlyphs;
        use flare_settings::look::LookStore;
        let f = fx();
        f.start_restore_and_wait();
        let grid = Rc::new(RefCell::new(GridHost::new(
            Box::new(FakeGlyphs::new(8, 16)),
            GridPainter::Cpu,
        )));
        grid.borrow_mut().resize(648, 264, 1.0);
        f.ctl
            .borrow_mut()
            .set_font_ctl(FontCtl::new(LookStore::new(f.prefs.clone()), grid));
        let a = f.ctl.borrow_mut().open_folder("C:\\za");
        let b = f.ctl.borrow_mut().open_folder("C:\\zb");
        f.wait_made("C:\\za");
        f.wait_made("C:\\zb");
        let size_of = |cwd: &str| {
            f.sp.made()
                .iter()
                .find(|m| m.spec().cwd == cwd)
                .map(|m| m.size())
                .unwrap()
        };
        let settle = || std::thread::sleep(Duration::from_millis(150));
        settle();
        let base = size_of("C:\\za");
        assert!(f.ctl.borrow_mut().zoom_front(1.0));
        assert!(f.ctl.borrow_mut().zoom_front(1.0));
        settle();
        let zoomed = size_of("C:\\zb");
        assert_ne!(zoomed, base, "the front tab follows its zoom");
        assert_eq!(size_of("C:\\za"), base, "a background shell is not touched");
        // Back to A: B, now in the background, keeps its zoomed grid.
        f.ctl.borrow_mut().model.pick(&a);
        f.ctl.borrow_mut().refresh();
        settle();
        assert_eq!(size_of("C:\\za"), base);
        assert_eq!(
            size_of("C:\\zb"),
            zoomed,
            "no reflow of a hidden zoomed shell"
        );
        f.ctl.borrow_mut().model.pick(&b);
        f.ctl.borrow_mut().refresh();
        settle();
        assert_eq!(size_of("C:\\zb"), zoomed);
        assert_eq!(size_of("C:\\za"), base, "the unzoomed shell keeps its size");
    }

    #[test]
    fn a_pick_swaps_the_grid_source_once() {
        let f = fx();
        let a = f.ctl.borrow_mut().open_folder("C:\\a");
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.wait_made("C:\\a");
        f.wait_made("C:\\b");
        assert_eq!(f.ctl.borrow().model().active().unwrap().id, b);
        // Resizes run on a helper thread now (rule 34), so a start's own size lands late: wait until the
        // counts hold still before taking them.
        let count = || -> Vec<usize> { f.sp.made().iter().map(|m| m.resize_calls()).collect() };
        let mut resizes = count();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(60));
            let now = count();
            if now == resizes {
                break;
            }
            resizes = now;
        }
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
        std::thread::sleep(std::time::Duration::from_millis(60));
        let after = count();
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
        std::fs::create_dir(&sub).unwrap(); // sync-ok: test code only, never the app
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
    fn the_end_of_life_stops_the_handoff_server_before_anything_slow() {
        let f = fx();
        f.start_restore_and_wait();
        let stops = Rc::new(Cell::new(0));
        let s = stops.clone();
        f.ctl
            .borrow_mut()
            .set_handoff_stop(Rc::new(move || s.set(s.get() + 1)));
        f.ctl.borrow_mut().end();
        assert_eq!(stops.get(), 1, "a late launch must find no pipe");
        f.ctl.borrow_mut().end();
        assert_eq!(stops.get(), 1, "once");
    }

    #[test]
    fn a_handoff_before_the_restore_is_queued() {
        let f = fx_with(Opts {
            saved: vec![("C:\\kept", None)],
            ..Opts::default()
        });
        let sub = f.dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap(); // sync-ok: test code only, never the app
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
        let raw = std::fs::read_to_string(f.dir.path().join("window.json")).ok()?; // sync-ok: test code only, never the app
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
        // Each share report gets its own drain, so a briefly adopted one would show.
        for report in [
            &b"\x1b]9;9;\\\\evil\\share\x07"[..],
            b"\x1b]9;9;\\/evil/share\x07",
            b"\x1b]9;9;\\\\?\\UNC\\evil\\share\x07",
        ] {
            m.feed(report);
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(150) {
                f.pump();
                assert_eq!(f.cwds(), ["C:\\a"], "a share report was adopted");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
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
        f.h.comp.invoke_menu_picked(1); // the recent folder C:\a, under "Choose a folder…"
        assert_eq!(f.rows(), 2);
    }

    #[test]
    fn an_open_menu_takes_escape_and_leaves_every_other_key_to_the_shell() {
        let f = fx();
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        f.wait_made("C:\\a");
        f.ctl.borrow_mut().open_tab_menu(&id, 10.0, 10.0);
        assert!(f.h.comp.get_menu_open());
        // A letter types into the shell and the menu stays up, as in Prism Terminal.
        assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Pty);
        assert!(f.h.comp.get_menu_open());
        // Escape shuts the menu and the shell never sees it.
        assert_eq!(
            f.key(KeyEvent::new("Escape", "Escape")),
            KeyOutcome::Consumed
        );
        assert!(!f.h.comp.get_menu_open());
        // With no menu, Escape is the shell's again.
        assert_eq!(f.key(KeyEvent::new("Escape", "Escape")), KeyOutcome::Pty);
        // The window losing the keyboard shuts a menu too.
        f.ctl.borrow_mut().open_plus_menu(10.0, 10.0);
        assert!(f.h.comp.get_menu_open());
        f.ctl.borrow_mut().on_focus_lost();
        assert!(!f.h.comp.get_menu_open());
    }

    // ---- the terminal's input wired into the window (M5, Task 24) ----------------------------------

    const KEY_C_RU: &str = "\u{441}"; // the Cyrillic letter on the C key

    /// One live shell tab (`C:\a`), its handle and its fake pty.
    fn live(opts: Opts) -> (Fx, Arc<TermHandle>, Arc<flare_term::fake::FakeState>) {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            ..opts
        });
        f.start_restore_and_wait();
        f.wait_made("C:\\a");
        let h = f.ctl.borrow().front_handle().expect("a live tab in front");
        f.pump_until("the shell", |_| h.shell_id().is_some());
        let fake = f.made_with("C:\\a")[0].clone();
        (f, h, fake)
    }

    fn feed(h: &TermHandle, s: &str) {
        h.with_emu(|e| {
            e.feed(s.as_bytes());
        });
    }

    /// A pointer position in css px (scale 1) for a cell, relative to the grid area.
    fn cell_css(col: usize, row: usize, right: bool) -> (f32, f32) {
        let m = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0);
        let x = m.padding as f32
            + col as f32 * m.cell_w as f32
            + if right { 0.75 } else { 0.25 } * m.cell_w as f32;
        let y = m.padding as f32 + row as f32 * m.cell_h as f32 + m.cell_h as f32 / 2.0;
        (x, y)
    }

    impl Fx {
        fn ptr(&self, kind: i32, button: i32, at: (f32, f32)) {
            self.h
                .comp
                .invoke_grid_pointer(kind, button, at.0, at.1, false, false, false);
        }

        fn drag_cols(&self, row: usize, a: usize, b: usize) {
            self.ptr(0, 0, cell_css(a, row, false));
            self.ptr(2, 0, cell_css(b, row, false));
            self.ptr(1, 0, cell_css(b, row, false));
        }

        fn clip_text(&self) -> Option<String> {
            self.clip.read_text()
        }
    }

    #[test]
    fn ctrl_f_opens_find_at_a_prompt_and_again_closes_it() {
        let (f, _h, _) = live(Opts::default());
        assert!(!f.h.comp.get_find_open());
        assert_eq!(f.key(ctrl("f", "KeyF")), KeyOutcome::Consumed);
        assert!(f.h.comp.get_find_open());
        assert_eq!(f.key(ctrl("f", "KeyF")), KeyOutcome::Consumed);
        assert!(!f.h.comp.get_find_open());
    }

    #[test]
    fn ctrl_f_is_the_programs_in_a_full_screen_program() {
        // The alternate screen.
        let (f, h, _) = live(Opts::default());
        feed(&h, "\x1b[?1049h");
        assert_eq!(f.key(ctrl("f", "KeyF")), KeyOutcome::Pty);
        assert!(!f.h.comp.get_find_open());
        // The mouse alone (a program that asked for it) is enough.
        let (f, h, _) = live(Opts::default());
        feed(&h, "\x1b[?1000h");
        assert!(!h.mode().alt_screen);
        assert_eq!(f.key(ctrl("f", "KeyF")), KeyOutcome::Pty);
        assert!(!f.h.comp.get_find_open());
    }

    #[test]
    fn ctrl_shift_f_always_finds() {
        let (f, h, _) = live(Opts::default());
        feed(&h, "\x1b[?1049h");
        assert_eq!(
            f.key(KeyEvent::new("F", "KeyF").ctrl().shift()),
            KeyOutcome::Consumed
        );
        assert!(f.h.comp.get_find_open());
    }

    #[test]
    fn keys_go_to_the_find_input_while_it_has_focus_but_chords_still_act() {
        let (f, _h, _) = live(Opts::default());
        f.key(ctrl("f", "KeyF"));
        assert!(f.h.comp.get_find_open());
        f.h.comp.invoke_focus_find();
        assert!(f.h.comp.get_find_input_focused());
        assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Slint);
        let rows = f.rows();
        assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
        assert_eq!(f.rows(), rows + 1);
    }

    #[test]
    fn ctrl_c_by_physical_key_copies_a_selection() {
        let (f, h, fake) = live(Opts::default());
        feed(&h, "hello world");
        f.drag_cols(0, 0, 5);
        // The Russian layout: the letter is not "c", the physical key is.
        let ev = KeyEvent::new(KEY_C_RU, "KeyC").ctrl();
        assert_eq!(f.key(ev.clone()), KeyOutcome::Consumed);
        f.pump_until("the copy", |f| f.clip_text().as_deref() == Some("hello"));
        f.pump_until("the badge", |f| f.h.comp.get_copied_phase() == 1);
        assert!(!fake.written().contains(&3), "a copy is not the interrupt");
        // The selection is gone, so the same key is the interrupt now.
        assert_eq!(f.key(ev.clone()), KeyOutcome::Pty);
        assert!(h.send_key(&ev));
        until_written(&fake, |w| w.contains(&3));
    }

    fn until_written(fake: &flare_term::fake::FakeState, ok: impl Fn(&[u8]) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if ok(&fake.written()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the pty never got the bytes: {:?}", fake.written());
    }

    #[test]
    fn ctrl_v_pastes_once() {
        let (f, h, fake) = live(Opts::default());
        feed(&h, "\x1b[?2004h");
        f.clip.set_text("a\nb");
        assert_eq!(f.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        f.pump_until("the paste", |_| fake.written() == b"\x1b[200~a\rb\x1b[201~");
        // Nothing more arrives later: one key is one paste.
        std::thread::sleep(Duration::from_millis(60));
        f.pump();
        assert_eq!(fake.written(), b"\x1b[200~a\rb\x1b[201~");
    }

    #[test]
    fn a_term_message_for_a_tab_that_is_no_longer_in_front_is_dropped() {
        let (f, _h, fake) = live(Opts::default());
        f.clip.set_text("late");
        assert_eq!(f.key(ctrl("v", "KeyV")), KeyOutcome::Consumed);
        // Before the worker's answer is handled, another tab comes to the front.
        f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        std::thread::sleep(Duration::from_millis(80));
        f.pump();
        assert!(fake.written().is_empty(), "{:?}", fake.written());
        let b = f.made_with("C:\\b");
        assert!(b.iter().all(|m| m.written().is_empty()));
    }

    #[test]
    fn a_right_click_on_the_strip_calls_no_system_menu() {
        let f = fx_with(Opts::default());
        let before = system_menu_requests();
        for x in [300.0, 500.0, 700.0] {
            f.h.right_click(x, BAR_H / 2.0);
        }
        assert_eq!(system_menu_requests(), before);
    }

    #[test]
    fn the_system_menu_opens_after_the_controller_is_let_go() {
        // The menu runs its own message loop. Opened inside on_key, the controller stays borrowed
        // for as long as the menu is up, and every message for it (shell output, timers) finds it
        // busy and is posted again at once: a spin. The menu is shown from a zero timer instead.
        let src = include_str!("appctl.rs");
        let at = src
            .find("pub fn open_system_menu")
            .expect("the method exists");
        let body = &src[at..at + src[at..].find("\n    }").expect("its end")];
        assert!(
            !body.contains("sysmenu::show_at_top_left(hwnd);"),
            "the menu must not open while the controller is borrowed"
        );
        assert!(body.contains("Timer::single_shot"), "the menu is deferred");
    }

    #[test]
    fn alt_space_asks_for_the_system_menu_once() {
        let f = fx_with(Opts::default());
        let before = system_menu_requests();
        let alt_space = KeyEvent::new(" ", "Space").alt();
        assert_eq!(f.key(alt_space), KeyOutcome::Consumed);
        assert_eq!(system_menu_requests(), before + 1);
    }

    #[test]
    fn a_front_change_closes_the_menu_and_the_find_bar() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", None)],
            ..Opts::default()
        });
        f.start_restore_and_wait();
        f.wait_made("C:\\a");
        f.wait_made("C:\\b");
        f.key(ctrl("f", "KeyF"));
        assert!(f.h.comp.get_find_open());
        f.ptr(0, 2, cell_css(2, 1, false));
        f.ptr(1, 2, cell_css(2, 1, false));
        assert!(f.h.comp.get_menu_open(), "the right release opens the menu");
        let other = f.ctl.borrow().model().state.tabs[1].id.clone();
        {
            let mut c = f.ctl.borrow_mut();
            c.model_mut().pick(&other);
            c.refresh();
        }
        assert!(!f.h.comp.get_find_open());
        assert!(!f.h.comp.get_menu_open());
    }

    #[test]
    fn a_drop_over_the_terminal_types_and_over_the_strip_opens_a_tab() {
        let (f, _h, fake) = live(Opts::default());
        let folder = f.dir.path().join("proj");
        std::fs::create_dir(&folder).unwrap(); // sync-ok: test code only, never the app
        // Over the terminal: a quoted path, typed after the quiet time, never Enter.
        f.ctl
            .borrow_mut()
            .on_dropped_path(PathBuf::from("C:\\My Dir\\a b.txt"), (500.0, 300.0));
        f.clock.set(f.clock.get() + 1000);
        f.ctl.borrow_mut().tick();
        until_written(&fake, |w| !w.is_empty());
        let typed = String::from_utf8(fake.written()).unwrap();
        assert!(typed.contains("C:\\My Dir\\a b.txt"), "{typed:?}");
        assert!(!typed.contains('\r') && !typed.contains('\n'), "{typed:?}");
        // Over the tab row: a tab opens in that folder.
        let rows = f.rows();
        f.ctl.borrow_mut().on_dropped_path(folder, (100.0, 10.0));
        f.pump_until("the new tab", |f| f.rows() == rows + 1);
    }

    #[test]
    fn a_drop_over_the_side_panel_types_nothing_into_the_shell() {
        let (f, _h, fake) = live(Opts::default());
        show_text(&f, "p.txt", "hello");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        f.h.advance(0);
        let slot_left = f.h.comp.get_grid_left() + f.h.comp.get_panel_slot_x();
        f.ctl.borrow_mut().on_dropped_path(
            PathBuf::from("C:\\x\\over_the_panel.png"),
            (slot_left + 30.0, 300.0),
        );
        f.clock.set(f.clock.get() + 1000);
        f.ctl.borrow_mut().tick();
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test code only
        f.pump();
        assert!(
            fake.written().is_empty(),
            "a path was typed into the shell: {:?}",
            String::from_utf8_lossy(&fake.written())
        );
        // Over the terminal part of the window it is typed as before.
        f.ctl.borrow_mut().on_dropped_path(
            PathBuf::from("C:\\x\\over_the_grid.png"),
            (slot_left - 100.0, 300.0),
        );
        f.clock.set(f.clock.get() + 1000);
        f.ctl.borrow_mut().tick();
        until_written(&fake, |w| !w.is_empty());
    }

    #[test]
    fn a_cmd_tab_with_no_requested_shell_quotes_a_drop_for_cmd() {
        let cmd = ShellDef {
            id: "cmd".to_string(),
            name: "Command Prompt".to_string(),
            exe: "cmd.exe".to_string(),
            args: Vec::new(),
        };
        let (f, h, fake) = live(Opts {
            shells: Some(vec![cmd]),
            ..Opts::default()
        });
        assert_eq!(h.shell_id().as_deref(), Some("cmd"));
        f.ctl
            .borrow_mut()
            .on_dropped_path(PathBuf::from("C:\\a&b.txt"), (500.0, 300.0));
        f.clock.set(f.clock.get() + 1000);
        f.ctl.borrow_mut().tick();
        until_written(&fake, |w| !w.is_empty());
        assert_eq!(fake.written(), b"\"C:\\a&b.txt\"");
    }

    #[test]
    fn a_bell_asks_the_taskbar_to_flash_and_a_burst_asks_once() {
        let (f, _h, fake) = live(Opts::default());
        let count = Rc::new(std::cell::Cell::new(0_u32));
        {
            let c = Rc::clone(&count);
            f.ctl
                .borrow_mut()
                .set_flash(Rc::new(move || c.set(c.get() + 1)));
        }
        fake.feed(b"\x07");
        f.pump_until("the flash", |_| count.get() == 1);
        // Ten more at once: the handle's one-a-second gate lets none of them through.
        fake.feed(&[7_u8; 10]);
        for _ in 0..20 {
            f.pump();
        }
        assert_eq!(count.get(), 1, "a burst is one flash");
    }

    #[test]
    fn clipboard_writes_from_osc_52_reach_the_port() {
        let (f, _h, fake) = live(Opts::default());
        fake.feed(b"\x1b]52;c;T1NDNTIgw6Y=\x07");
        f.pump_until("the copy", |f| {
            f.clip_text().as_deref() == Some("OSC52 \u{e6}")
        });
        f.pump_until("the badge", |f| f.h.comp.get_copied_phase() == 1);
    }

    #[test]
    fn under_e2e_the_clipboard_is_in_memory_and_openers_record() {
        let b = Backends::new(true);
        assert!(b.recording.is_some());
        b.clip_port.write_text("kept in memory", 100).unwrap();
        assert_eq!(b.clip_port.read_text().as_deref(), Some("kept in memory"));
        let open = b.open_url();
        open("https://example.com/x");
        let urls = b.urls.as_ref().unwrap().lock().unwrap().clone();
        assert_eq!(urls, ["https://example.com/x"]);
        // The test hook's url.log probe reads the same opens.
        assert!(
            crate::e2e_probes::input_b::recorded_urls()
                .contains(&"https://example.com/x".to_string()),
            "the probe sees what the opener recorded"
        );
        // A click on a printed url reaches the recorder, never the desktop.
        let (f, h, _) = live(Opts::default());
        feed(&h, "see https://example.com/x now");
        f.ctl.borrow().decor_inputs().borrow_mut().decor(&h);
        f.ctl.borrow_mut().tick();
        f.ptr(0, 0, cell_css(8, 0, false));
        f.ptr(1, 0, cell_css(8, 0, false));
        assert_eq!(f.log.borrow().urls, ["https://example.com/x"]);
        assert!(f.openers.records().is_empty());
    }

    #[test]
    fn a_path_click_opens_through_the_openers_port() {
        let (f, h, _) = live(Opts::default());
        // With previews off a text file goes to Windows as it always did.
        f.prefs.set("flare.window.openPreviews", "0");
        f.pump();
        let file = f.dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap(); // sync-ok: test code only, never the app
        let dir = f.dir.path().to_string_lossy().into_owned();
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl.borrow_mut().model_mut().set_cwd(&id, &dir);
        feed(&h, "a.txt");
        for _ in 0..8 {
            f.ctl.borrow().decor_inputs().borrow_mut().decor(&h);
            f.clock.set(f.clock.get() + 60);
            f.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(30));
            f.pump();
        }
        f.ctl.borrow().decor_inputs().borrow_mut().decor(&h);
        f.ptr(0, 0, cell_css(1, 0, false));
        f.ptr(1, 0, cell_css(1, 0, false));
        let t = Instant::now();
        while f.openers.records().is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let want = file.to_string_lossy().into_owned();
        assert_eq!(f.openers.records(), [OpenedRecord::Open(want)]);
    }

    // ---- the panels (Surfaces stage 1) --------------------------------------------------------

    /// Feeds `word` on its own row, lets the path questions settle, and left-clicks it. `row` is
    /// the row the word lands on (the rows before it were fed by earlier calls).
    fn click_word(f: &Fx, h: &TermHandle, dir: &str, word: &str, row: usize) {
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl.borrow_mut().model_mut().set_cwd(&id, dir);
        feed(h, &format!("{word}\r\n"));
        for _ in 0..8 {
            f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
            f.clock.set(f.clock.get() + 60);
            f.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(30));
            f.pump();
        }
        f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
        f.ptr(0, 0, cell_css(1, row, false));
        f.ptr(1, 0, cell_css(1, row, false));
    }

    /// Like [`click_word`] with the right button: opens the terminal's menu on the word.
    fn right_click_word(f: &Fx, h: &TermHandle, dir: &str, word: &str, row: usize) {
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl.borrow_mut().model_mut().set_cwd(&id, dir);
        feed(h, &format!("{word}\r\n"));
        for _ in 0..8 {
            f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
            f.clock.set(f.clock.get() + 60);
            f.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(30));
            f.pump();
        }
        f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
        f.ptr(0, 2, cell_css(1, row, false));
        f.ptr(1, 2, cell_css(1, row, false));
    }

    fn menu_labels(f: &Fx) -> Vec<String> {
        use slint::Model;
        f.h.comp
            .get_menu_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect()
    }

    #[test]
    fn show_in_side_panel_on_a_text_file_menu_opens_the_preview_and_opens_nothing_in_windows() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(
            f.dir.path().join("a.txt"),
            "one
two",
        )
        .unwrap(); // sync-ok: test code only
        right_click_word(&f, &h, &dir, "a.txt", 0);
        assert!(f.h.comp.get_menu_open());
        let labels = menu_labels(&f);
        assert_eq!(
            labels[..3],
            ["Open", "Show in side panel", "Show in Explorer"],
            "{labels:?}"
        );
        let i = labels
            .iter()
            .position(|l| l == "Show in side panel")
            .unwrap();
        f.h.comp.invoke_menu_picked(i as i32);
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "a.txt");
        assert!(f.openers.records().is_empty(), "{:?}", f.openers.records());
        assert!(
            !f.h.comp.get_panel_has_focus(),
            "the keys stay in the terminal"
        );
    }

    #[test]
    fn show_in_side_panel_works_even_when_plain_clicks_open_files_in_windows() {
        let (f, h, _) = live(Opts::default());
        f.prefs.set("flare.window.openPreviews", "0");
        f.pump();
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("a.txt"), "one").unwrap(); // sync-ok: test code only
        right_click_word(&f, &h, &dir, "a.txt", 0);
        let labels = menu_labels(&f);
        let i = labels
            .iter()
            .position(|l| l == "Show in side panel")
            .expect("an explicit ask is offered whatever the click setting says");
        f.h.comp.invoke_menu_picked(i as i32);
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        assert!(f.openers.records().is_empty(), "{:?}", f.openers.records());
    }

    #[test]
    fn show_in_side_panel_on_a_picture_menu_and_never_for_a_script_or_a_folder() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        write_png(&f.dir.path().join("p.png"), 20, 10);
        std::fs::write(f.dir.path().join("run.bat"), "x").unwrap(); // sync-ok: test code only
        std::fs::create_dir(f.dir.path().join("sub")).unwrap(); // sync-ok: test code only
        right_click_word(&f, &h, &dir, "p.png", 0);
        let i = menu_labels(&f)
            .iter()
            .position(|l| l == "Show in side panel")
            .expect("a picture offers it");
        f.h.comp.invoke_menu_picked(i as i32);
        f.pump_until("the picture", |f| f.h.comp.get_panel_kind() == 1);
        // A script and a folder do not.
        right_click_word(&f, &h, &dir, "run.bat", 1);
        assert!(f.h.comp.get_menu_open());
        assert!(!menu_labels(&f).contains(&"Show in side panel".to_string()));
        f.h.comp.invoke_menu_dismissed();
        right_click_word(&f, &h, &dir, "sub/", 2);
        assert!(!menu_labels(&f).contains(&"Show in side panel".to_string()));
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        let file = std::fs::File::create(path).unwrap(); // sync-ok: test code only, never the app
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let px: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 100, 50, 255]).collect();
        wr.write_image_data(&px).unwrap();
    }

    fn shift_ctrl(key: &str, code: &str) -> KeyEvent {
        KeyEvent::new(key, code).ctrl().shift()
    }

    fn rows_in_panel(f: &Fx) -> usize {
        use slint::Model;
        f.h.comp.get_panel_lines().row_count()
    }

    #[test]
    fn a_click_on_a_text_file_shows_it_in_the_panel_and_hands_nothing_to_windows() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("a.txt"), "one\ntwo\nthree").unwrap(); // sync-ok: test code only
        click_word(&f, &h, &dir, "a.txt", 0);
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        assert!(f.h.comp.get_panel_open());
        assert_eq!(f.h.comp.get_panel_title().as_str(), "a.txt");
        assert_eq!(f.h.comp.get_panel_sub().as_str(), "3 lines, 13 B");
        assert_eq!(rows_in_panel(&f), 3);
        assert!(f.openers.records().is_empty(), "{:?}", f.openers.records());
        // The click left the keys in the terminal.
        assert!(!f.h.comp.get_panel_has_focus());
    }

    #[test]
    fn a_click_on_a_picture_shows_it_with_its_size() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        write_png(&f.dir.path().join("p.png"), 40, 30);
        click_word(&f, &h, &dir, "p.png", 0);
        f.pump_until("the picture", |f| f.h.comp.get_panel_kind() == 1);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "p.png");
        assert_eq!(f.h.comp.get_panel_sub().as_str(), "40 x 30");
        let size = f.h.comp.get_panel_picture().size();
        assert_eq!((size.width, size.height), (40, 30));
        assert!(
            !f.h.comp.get_panel_alpha(),
            "an opaque picture needs no board"
        );
        assert!(f.openers.records().is_empty());
    }

    #[test]
    fn a_picture_that_is_not_one_answers_with_a_reason_in_the_panel() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("bad.png"), b"not a picture").unwrap(); // sync-ok: test code only
        click_word(&f, &h, &dir, "bad.png", 0);
        f.pump_until("the reason", |f| {
            !f.h.comp.get_panel_note().is_empty()
                && f.h.comp.get_panel_note().as_str() != "Opening\u{2026}"
        });
        assert_eq!(f.h.comp.get_panel_kind(), 0);
        assert!(
            f.h.comp
                .get_panel_note()
                .contains("cannot read this picture")
        );
    }

    #[test]
    fn a_script_a_document_and_a_folder_never_open_a_panel() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("run.bat"), "x").unwrap(); // sync-ok: test code only
        std::fs::write(f.dir.path().join("d.pdf"), "x").unwrap(); // sync-ok: test code only
        std::fs::create_dir(f.dir.path().join("sub")).unwrap(); // sync-ok: test code only
        click_word(&f, &h, &dir, "run.bat", 0);
        click_word(&f, &h, &dir, "d.pdf", 1);
        click_word(&f, &h, &dir, "sub/", 2);
        let t = Instant::now();
        while f.openers.records().len() < 3 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        f.pump();
        let want = |n: &str| f.dir.path().join(n).to_string_lossy().into_owned();
        let rec = f.openers.records();
        assert!(
            rec.contains(&OpenedRecord::Reveal(want("run.bat"))),
            "{rec:?}"
        );
        assert!(rec.contains(&OpenedRecord::Open(want("d.pdf"))), "{rec:?}");
        assert!(
            rec.iter().any(
                |r| matches!(r, OpenedRecord::Folder(p) if p.trim_end_matches('\\') == want("sub"))
            ),
            "{rec:?}"
        );
        assert!(!rec.contains(&OpenedRecord::Open(want("run.bat"))));
        assert!(
            !f.h.comp.get_panel_open(),
            "none of them is shown in a panel"
        );
        assert_eq!(f.ctl.borrow().open_panels(), 0);
    }

    #[test]
    fn a_picture_named_like_one_that_is_a_script_is_not_shown() {
        // A name that says png but a file that is not on the allowlist as the system names it
        // cannot be made without a link; the extension check on the clicked name alone is the
        // part a plain file can exercise: a .png.exe is a program, not a picture.
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("pic.png.exe"), "x").unwrap(); // sync-ok: test code only
        click_word(&f, &h, &dir, "pic.png.exe", 0);
        let t = Instant::now();
        while f.openers.records().is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let want = f
            .dir
            .path()
            .join("pic.png.exe")
            .to_string_lossy()
            .into_owned();
        assert_eq!(f.openers.records(), [OpenedRecord::Reveal(want)]);
        assert!(!f.h.comp.get_panel_open());
    }

    /// Prints a word, lights it up as a link and leaves the pointer on it.
    fn rest_on_word(f: &Fx, h: &TermHandle, dir: &str, word: &str) {
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl.borrow_mut().model_mut().set_cwd(&id, dir);
        feed(h, &format!("{word}\r\n"));
        for _ in 0..8 {
            f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
            f.clock.set(f.clock.get() + 60);
            f.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(30));
            f.pump();
        }
        f.ctl.borrow().decor_inputs().borrow_mut().decor(h);
        f.ptr(2, 0, cell_css(1, 0, false));
    }

    fn thumb_png(f: &Fx, name: &str) {
        image::RgbaImage::from_pixel(64, 32, image::Rgba([9, 200, 30, 255]))
            .save(f.dir.path().join(name))
            .unwrap();
    }

    #[test]
    fn resting_on_a_picture_path_shows_the_bubble_under_the_line_and_leaving_the_grid_ends_it() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        thumb_png(&f, "p.png");
        f.pump();
        rest_on_word(&f, &h, &dir, "p.png");
        f.clock.set(f.clock.get() + 400);
        f.pump_until("the thumbnail", |f| {
            f.ctl.borrow_mut().tick();
            f.h.comp.get_thumb_visible()
        });
        assert_eq!(f.h.comp.get_thumb_caption().as_str(), "p.png  64 x 32");
        assert_eq!(f.h.comp.get_thumb_pic_w(), 64.0);
        assert!(
            f.h.comp.get_thumb_y() >= f.h.comp.get_grid_top() + 16.0,
            "under the line"
        );
        assert!(
            f.h.comp.get_thumb_x() + f.h.comp.get_thumb_w() <= f.h.comp.get_grid_right(),
            "inside the grid"
        );
        f.h.comp.invoke_grid_pointer_left();
        assert!(!f.h.comp.get_thumb_visible());
    }

    #[test]
    fn with_hover_thumbnails_off_resting_on_a_picture_path_shows_nothing() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        thumb_png(&f, "p.png");
        f.prefs.set("flare.window.hoverThumbs", "0");
        f.pump();
        rest_on_word(&f, &h, &dir, "p.png");
        f.clock.set(f.clock.get() + 400);
        for _ in 0..10 {
            f.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(20));
            f.pump();
        }
        assert!(!f.h.comp.get_thumb_visible());
    }

    #[test]
    fn a_key_takes_the_thumbnail_down() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        thumb_png(&f, "p.png");
        f.pump();
        rest_on_word(&f, &h, &dir, "p.png");
        f.clock.set(f.clock.get() + 400);
        f.pump_until("the thumbnail", |f| {
            f.ctl.borrow_mut().tick();
            f.h.comp.get_thumb_visible()
        });
        f.ctl.borrow_mut().term.hide_thumb();
        assert!(!f.h.comp.get_thumb_visible());
    }

    #[test]
    fn with_previews_off_a_text_file_goes_to_windows_and_turning_them_on_again_shows_it() {
        let (f, h, _) = live(Opts::default());
        let dir = f.dir.path().to_string_lossy().into_owned();
        std::fs::write(f.dir.path().join("a.txt"), "x").unwrap(); // sync-ok: test code only
        f.prefs.set("flare.window.openPreviews", "0");
        f.pump();
        click_word(&f, &h, &dir, "a.txt", 0);
        let t = Instant::now();
        while f.openers.records().is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(f.openers.records().len(), 1);
        assert!(!f.h.comp.get_panel_open());
        f.prefs.set("flare.window.openPreviews", "1");
        f.pump();
        click_word(&f, &h, &dir, "a.txt", 1);
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(
            f.openers.records().len(),
            1,
            "the second click opened nothing in Windows"
        );
    }

    /// Shows a text file in the front tab's panel, as a resolved click does.
    fn show_text(f: &Fx, name: &str, body: &str) -> String {
        let path = f.dir.path().join(name);
        std::fs::write(&path, body).unwrap(); // sync-ok: test code only
        let abs = path.to_string_lossy().into_owned();
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl
            .borrow_mut()
            .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
        abs
    }

    #[test]
    fn each_tab_has_its_own_panel_and_the_front_tab_decides_what_shows() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        show_text(&f, "one.txt", "1");
        f.pump_until("the first panel", |f| f.h.comp.get_panel_kind() == 2);
        // A second tab has none.
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        assert_eq!(f.ctl.borrow().front_id().as_deref(), Some(b.as_str()));
        assert!(!f.h.comp.get_panel_open(), "the new tab has no panel");
        assert_eq!(f.h.comp.get_panel_kind(), 0);
        assert_eq!(
            rows_in_panel(&f),
            0,
            "nothing is kept on show for a tab behind"
        );
        // Back to the first: its panel reads its file again.
        f.ctl.borrow_mut().model_mut().pick(&a);
        f.ctl.borrow_mut().refresh();
        f.pump_until("the panel back", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "one.txt");
    }

    #[test]
    fn closing_a_tab_forgets_its_panel() {
        let (f, _h, _) = live(Opts::default());
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        show_text(&f, "b.txt", "b");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.ctl.borrow().open_panels(), 1);
        f.ctl.borrow_mut().request_close(&b);
        f.pump();
        assert_eq!(f.ctl.borrow().open_panels(), 0);
        assert!(!f.h.comp.get_panel_open());
        assert_eq!(rows_in_panel(&f), 0);
    }

    #[test]
    fn the_close_button_and_the_toggle_chord_close_and_bring_the_panel_back() {
        let (f, _h, _) = live(Opts::default());
        // Nothing to toggle: the chord is still the app's, so the shell never hears it.
        assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
        assert!(!f.h.comp.get_panel_open());
        show_text(&f, "a.txt", "a\nb");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
        assert!(!f.h.comp.get_panel_open());
        assert_eq!(rows_in_panel(&f), 0, "a closed panel holds no rows");
        assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
        f.pump_until("the panel again", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(rows_in_panel(&f), 2);
        f.h.comp.invoke_panel_close();
        assert!(!f.h.comp.get_panel_open());
        assert_eq!(f.ctl.borrow().open_panels(), 0);
    }

    #[test]
    fn the_focus_chord_moves_the_keys_both_ways_and_escape_gives_them_back() {
        let (f, _h, _) = live(Opts::default());
        f.h.activate();
        // No panel: nothing moves, and the shell does not hear it.
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        assert!(!f.h.comp.get_panel_has_focus());
        show_text(&f, "a.txt", "a\nb\nc");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert!(!f.h.comp.get_panel_has_focus());
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        // A plain key now goes to Slint, not the shell.
        assert_eq!(f.key(KeyEvent::new("j", "KeyJ")), KeyOutcome::Slint);
        // The chord again: the terminal has them.
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_has_focus());
        assert_ne!(f.key(KeyEvent::new("j", "KeyJ")), KeyOutcome::Slint);
        // Escape in the panel does the same.
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        f.h.key_text(&String::from(char::from(slint::platform::Key::Escape)));
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_has_focus());
    }

    #[test]
    fn the_keys_return_to_the_terminal_when_the_panel_they_were_in_closes() {
        let (f, _h, _) = live(Opts::default());
        f.h.activate();
        show_text(&f, "a.txt", "a");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_has_focus());
        // Bringing the panel back does not hand it the keys again.
        assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_has_focus());
    }

    #[test]
    fn the_keys_return_to_the_terminal_when_the_tab_changes_or_the_close_button_is_pressed() {
        let (f, _h, _) = live(Opts::default());
        f.h.activate();
        show_text(&f, "a.txt", "a");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        // A tab with no panel comes to the front: the keys are the terminal's again.
        let _b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.ctl.borrow_mut().refresh();
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_shown());
        assert!(!f.h.comp.get_panel_has_focus());
        assert_ne!(f.key(KeyEvent::new("j", "KeyJ")), KeyOutcome::Slint);
        // The same through the close button.
        show_text(&f, "c.txt", "c");
        f.pump_until("the panel on the new tab", |f| {
            f.h.comp.get_panel_kind() == 2
        });
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        f.h.comp.invoke_panel_close();
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_has_focus());
        assert_ne!(f.key(KeyEvent::new("j", "KeyJ")), KeyOutcome::Slint);
    }

    #[test]
    fn the_keys_return_to_the_terminal_when_the_tab_changes_to_one_with_a_panel_too() {
        let (f, _h, _) = live(Opts::default());
        f.h.activate();
        let a = f.ctl.borrow().front_id().unwrap();
        show_text(&f, "a.txt", "a");
        f.pump_until("the panel on a", |f| f.h.comp.get_panel_kind() == 2);
        let b = f.ctl.borrow_mut().open_folder(r"C:\b");
        f.ctl.borrow_mut().refresh();
        show_text(&f, "b.txt", "b");
        f.pump_until("the panel on b", |f| f.h.comp.get_panel_kind() == 2);
        f.ctl.borrow_mut().model_mut().pick(&a);
        f.ctl.borrow_mut().refresh();
        f.pump_until("the panel back on a", |f| {
            f.h.comp.get_panel_title().as_str() == "a.txt"
        });
        assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_has_focus());
        // The other tab has a panel too: it is still shown, and the keys are the terminal's.
        f.ctl.borrow_mut().model_mut().pick(&b);
        f.ctl.borrow_mut().refresh();
        f.h.advance(0);
        f.pump_until("the panel on b again", |f| {
            f.h.comp.get_panel_title().as_str() == "b.txt"
        });
        assert!(f.h.comp.get_panel_shown());
        assert!(!f.h.comp.get_panel_has_focus());
        assert_ne!(f.key(KeyEvent::new("j", "KeyJ")), KeyOutcome::Slint);
    }

    #[test]
    fn a_click_on_the_file_already_shown_reads_it_again() {
        let (f, _h, _) = live(Opts::default());
        let abs = show_text(&f, "a.txt", "one");
        f.pump_until("the first read", |f| rows_in_panel(f) == 1);
        // The agent rewrites the file; the next click shows what it says now.
        std::fs::write(&abs, "one\ntwo\nthree").unwrap(); // sync-ok: test code only
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl
            .borrow_mut()
            .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
        f.pump_until("the second read", |f| rows_in_panel(f) == 3);
    }

    #[test]
    fn a_file_that_was_missing_shows_when_it_exists_and_is_clicked_again() {
        let (f, _h, _) = live(Opts::default());
        let path = f.dir.path().join("later.txt");
        let abs = path.to_string_lossy().into_owned();
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl
            .borrow_mut()
            .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
        f.pump_until("the reason", |f| {
            f.h.comp.get_panel_note().as_str() == "The file is gone."
        });
        std::fs::write(&path, "now\nhere").unwrap(); // sync-ok: test code only
        f.ctl
            .borrow_mut()
            .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
        f.pump_until("the content", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(rows_in_panel(&f), 2);
    }

    #[test]
    fn a_restored_target_that_fails_the_clicks_checks_is_not_read() {
        // A folder named like a text file passes the string checks; the click's checks refuse it.
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            ..Opts::default()
        });
        let folder = f.dir.path().join("dir.txt");
        std::fs::create_dir(&folder).unwrap(); // sync-ok: test code only
        let file = f.dir.path().join("panels.json");
        let raw = format!(
            r#"{{"version":1,"tabs":[{{"cwd":"C:\\a","panel":{{"kind":"text","target":{}}}}}]}}"#,
            serde_json::Value::String(folder.to_string_lossy().into_owned())
        );
        std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
        f.ctl
            .borrow_mut()
            .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
                &file,
                Duration::from_millis(30),
            )));
        f.start_restore_and_wait();
        f.pump_until("the refusal", |f| {
            f.h.comp.get_panel_open() && f.h.comp.get_panel_note().as_str().contains("not allowed")
        });
        assert_eq!(rows_in_panel(&f), 0);
    }

    #[test]
    fn every_previewable_type_is_on_the_click_allowlist() {
        // A type dropped from the allowlist would silently stop previewing.
        for ext in flare_surfaces::preview::TEXT_EXTS
            .iter()
            .chain(flare_surfaces::preview::IMAGE_EXTS.iter())
        {
            assert!(
                flare_term::path_open::may_open(&format!("C:\\a\\x.{ext}")),
                "{ext}"
            );
        }
    }

    #[test]
    fn the_panel_chord_does_nothing_for_a_kind_this_build_cannot_draw() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            ..Opts::default()
        });
        let file = f.dir.path().join("panels.json");
        std::fs::write(
            &file,
            r#"{"version":1,"tabs":[{"cwd":"C:\\a","panel":{"kind":"mod:notes","target":"x"}}]}"#,
        )
        .unwrap(); // sync-ok: test code only
        f.ctl
            .borrow_mut()
            .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
                &file,
                Duration::from_millis(30),
            )));
        f.start_restore_and_wait();
        assert_eq!(f.ctl.borrow().open_panels(), 1, "it is open as stored");
        assert!(!f.ctl.borrow_mut().toggle_front_panel());
        assert_eq!(f.ctl.borrow().open_panels(), 1, "and the chord left it so");
    }

    // ---- wrapping a text preview (2026-10-22) ----------------------------------------------------

    fn long_body() -> String {
        format!("short one\n{}\nshort two\n", "word ".repeat(100).trim_end())
    }

    fn nums_in_panel(f: &Fx) -> Vec<i32> {
        use slint::Model;
        f.h.comp.get_panel_line_nums().iter().collect()
    }

    #[test]
    fn a_long_line_wraps_by_default_and_the_header_button_turns_it_off_and_on() {
        let (f, _h, _) = live(Opts::default());
        show_text(&f, "long.txt", &long_body());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        let wrapped = rows_in_panel(&f);
        assert!(wrapped > 3, "the long line took several rows ({wrapped})");
        assert!(f.h.comp.get_panel_wrapped());
        assert!(f.h.comp.get_panel_wrap_on());
        let nums = nums_in_panel(&f);
        assert_eq!(nums.len(), wrapped);
        assert_eq!(nums[0], 1);
        assert_eq!(
            *nums.last().unwrap(),
            3,
            "the last line keeps its own number"
        );
        assert_eq!(
            nums.iter().filter(|n| **n > 0).count(),
            3,
            "three lines, three numbers"
        );
        assert!(
            f.h.comp.get_panel_sub().as_str().starts_with("3 lines"),
            "the header counts lines, not rows"
        );
        // The button turns it off: the rows are the lines again, and the setting says so.
        f.h.comp.invoke_panel_wrap_toggled();
        f.pump();
        assert!(!flare_settings::term_prefs::wrap_previews(&f.prefs));
        assert_eq!(rows_in_panel(&f), 3);
        assert!(!f.h.comp.get_panel_wrapped());
        assert!(!f.h.comp.get_panel_wrap_on());
        assert!(nums_in_panel(&f).is_empty());
        // And on again.
        f.h.comp.invoke_panel_wrap_toggled();
        f.pump();
        assert!(flare_settings::term_prefs::wrap_previews(&f.prefs));
        assert_eq!(rows_in_panel(&f), wrapped);
        assert!(f.h.comp.get_panel_wrapped());
    }

    #[test]
    fn with_the_setting_off_a_file_opens_unwrapped_and_the_settings_switch_flips_it_too() {
        let (f, _h, _) = live(Opts::default());
        flare_settings::term_prefs::set_wrap_previews(&f.prefs, false);
        f.pump();
        show_text(&f, "long.txt", &long_body());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(rows_in_panel(&f), 3, "three lines, three rows");
        assert!(!f.h.comp.get_panel_wrapped());
        // The Settings page writes the same key; the panel follows it.
        flare_settings::term_prefs::set_wrap_previews(&f.prefs, true);
        f.pump();
        f.ctl.borrow_mut().refresh();
        assert!(rows_in_panel(&f) > 3);
    }

    #[test]
    fn the_sidebar_sliding_wraps_the_text_once_not_at_every_frame() {
        let (f, _h, _) = live(Opts::default());
        set_tab_layout(&f.prefs, TabLayout::Vertical);
        f.pump();
        show_text(&f, "long.txt", &long_body());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        f.h.advance(300);
        f.pump();
        let runs0 = f.ctl.borrow().panels.wrap_count();
        let cols0 = f.h.comp.get_panel_wrap_cols();
        f.ctl.borrow_mut().toggle_sidebar();
        let mut widths = std::collections::BTreeSet::new();
        for _ in 0..80 {
            f.clock.set(f.clock.get() + 16);
            f.h.advance(16);
            f.pump();
            widths.insert(f.h.comp.get_panel_wrap_cols());
        }
        f.h.advance(400);
        f.pump();
        assert_ne!(
            f.h.comp.get_panel_wrap_cols(),
            cols0,
            "the slide changed the page's width"
        );
        assert!(widths.len() > 2, "the width went through steps: {widths:?}");
        let runs = f.ctl.borrow().panels.wrap_count() - runs0;
        assert_eq!(runs, 1, "wrapped {runs} times for one slide");
    }

    /// Drags the splitter to `x + dx` and gives the window a frame.
    fn drag_to(f: &Fx, x: f32, y: f32, dx: f32) {
        f.h.move_to(x + dx, y);
        f.h.advance(0);
        f.pump();
    }

    #[test]
    fn a_splitter_drag_wraps_live_at_every_step_and_the_release_changes_nothing_more() {
        for scale in [1.0f32, 2.25] {
            let (f, _h, _) = live(Opts {
                scale,
                ..Opts::default()
            });
            show_text(&f, "many.txt", &many_long_lines());
            f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
            let rows0 = rows_in_panel(&f);
            let runs0 = f.ctl.borrow().panels.wrap_count();
            let x = f.h.comp.get_panel_splitter_x() + f.h.comp.get_panel_splitter_w() / 2.0;
            let y = f.h.comp.get_grid_top() + 200.0;
            f.h.move_to(x, y);
            f.h.press(x, y, slint::platform::PointerEventButton::Left);
            assert!(f.h.comp.get_panel_splitting(), "scale {scale}");
            // Step by step wider: the rows come down while the splitter is still held.
            let mut counts = vec![rows0];
            for k in 1..=12 {
                drag_to(&f, x, y, -25.0 * k as f32);
                assert!(f.h.comp.get_panel_splitting(), "scale {scale}: still held");
                assert_eq!(
                    f.h.comp.get_panel_wrapped_cols(),
                    f.h.comp.get_panel_wrap_cols(),
                    "scale {scale}, step {k}: the rows are wrapped to the width on show"
                );
                counts.push(rows_in_panel(&f));
            }
            let distinct: std::collections::BTreeSet<_> = counts.iter().collect();
            assert!(
                distinct.len() > 2
                    && counts[1..counts.len() - 1]
                        .iter()
                        .any(|c| *c != counts[0] && *c != counts[counts.len() - 1]),
                "scale {scale}: rows changed at intermediate widths, not only after the release: {counts:?}"
            );
            assert!(
                counts.windows(2).all(|w| w[1] <= w[0]),
                "scale {scale}: a wider slot never makes more rows: {counts:?}"
            );
            assert!(
                f.ctl.borrow().panels.wrap_count() > runs0 + 3,
                "scale {scale}: wraps happened mid-drag"
            );
            // And back, narrower: the rows go up again.
            let wide = rows_in_panel(&f);
            for k in (0..12).rev() {
                drag_to(&f, x, y, -25.0 * k as f32);
            }
            assert!(
                rows_in_panel(&f) > wide,
                "scale {scale}: narrower, more rows"
            );
            drag_to(&f, x, y, -150.0);
            let runs_at_release = f.ctl.borrow().panels.wrap_count();
            let before = rows_in_panel(&f);
            f.h.release(x - 150.0, y, slint::platform::PointerEventButton::Left);
            f.pump();
            assert_eq!(
                f.ctl.borrow().panels.wrap_count(),
                runs_at_release,
                "scale {scale}: the rows already fit the width, the release wraps nothing again"
            );
            assert_eq!(rows_in_panel(&f), before, "scale {scale}");
            assert_eq!(
                f.h.comp.get_panel_wrapped_cols(),
                f.h.comp.get_panel_wrap_cols(),
                "scale {scale}: exact after the release"
            );
            f.pump();
            f.ctl.borrow_mut().refresh();
            assert_eq!(f.ctl.borrow().panels.wrap_count(), runs_at_release);
        }
    }

    #[test]
    fn a_live_drag_keeps_the_line_at_the_top_in_place() {
        for scale in [1.0f32, 2.25] {
            let (f, _h, _) = live(Opts {
                scale,
                ..Opts::default()
            });
            show_text(&f, "many.txt", &many_long_lines());
            f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
            f.h.comp.invoke_panel_scroll_by(0.0, -18.0 * 700.0);
            f.h.advance(0);
            let line = top_line_number(&f);
            assert!(line > 100, "scale {scale}: scrolled into the file ({line})");
            let x = f.h.comp.get_panel_splitter_x() + f.h.comp.get_panel_splitter_w() / 2.0;
            let y = f.h.comp.get_grid_top() + 200.0;
            f.h.move_to(x, y);
            f.h.press(x, y, slint::platform::PointerEventButton::Left);
            let rows0 = rows_in_panel(&f);
            let mut rows_changed = false;
            for k in [1.0f32, 2.0, 3.0, 4.0, 3.0, 2.0, -1.0, -3.0] {
                drag_to(&f, x, y, -20.0 * k);
                f.h.advance(0);
                rows_changed |= rows_in_panel(&f) != rows0;
                assert_eq!(
                    top_line_number(&f),
                    line,
                    "scale {scale}: the same line stays at the top at step {k}"
                );
            }
            assert!(
                rows_changed,
                "scale {scale}: the rows re-wrapped mid-drag (settle-only code fails here)"
            );
            f.h.release(x, y, slint::platform::PointerEventButton::Left);
            f.pump();
            assert_eq!(
                top_line_number(&f),
                line,
                "scale {scale}: and after the release"
            );
        }
    }

    /// A text file over the live threshold, so a drag wraps it on a worker.
    fn big_body() -> String {
        format!("{}\n", "word ".repeat(300).trim_end()).repeat(900)
    }

    #[test]
    fn a_big_file_drag_never_stalls_the_ui_and_ends_exact() {
        for scale in [1.0f32, 2.25] {
            let (f, _h, _) = live(Opts {
                scale,
                ..Opts::default()
            });
            show_text(&f, "big.txt", &big_body());
            f.pump_until("the wrapped rows", |f| {
                f.h.comp.get_panel_kind() == 2 && f.h.comp.get_panel_wrapped()
            });
            let rows0 = rows_in_panel(&f);
            let x = f.h.comp.get_panel_splitter_x() + f.h.comp.get_panel_splitter_w() / 2.0;
            let y = f.h.comp.get_grid_top() + 200.0;
            f.h.move_to(x, y);
            f.h.press(x, y, slint::platform::PointerEventButton::Left);
            let mut worst = Duration::ZERO;
            let mut changed_mid_drag = false;
            let runs0 = f.ctl.borrow().panels.wrap_count();
            for k in 1..=40 {
                let t = Instant::now();
                f.h.move_to(x - 6.0 * k as f32, y);
                f.h.advance(0);
                f.pump();
                worst = worst.max(t.elapsed());
                changed_mid_drag |= rows_in_panel(&f) != rows0;
                // Long enough now and then for a worker's answer to land while the drag goes on.
                std::thread::sleep(Duration::from_millis(if k % 5 == 0 { 150 } else { 8 }));
            }
            // A step hands the work to a worker and goes on: no step waits for a wrap of the whole file.
            assert!(
                worst < Duration::from_millis(100),
                "scale {scale}: a drag step took {worst:?}"
            );
            // Fewer wraps than steps is fine (one in flight, the newest width next), but never none.
            let runs = f.ctl.borrow().panels.wrap_count() - runs0;
            assert!(
                (1..=40).contains(&runs),
                "scale {scale}: {runs} wraps for 40 steps"
            );
            f.h.release(x - 240.0, y, slint::platform::PointerEventButton::Left);
            f.pump_until("the exact final wrap", |f| {
                f.h.advance(0);
                f.h.comp.get_panel_wrapped()
                    && f.h.comp.get_panel_wrapped_cols() == f.h.comp.get_panel_wrap_cols()
            });
            assert!(
                changed_mid_drag,
                "scale {scale}: a worker's answer re-wrapped the rows while the drag was still held"
            );
            let lines: Vec<slint::SharedString> =
                big_body().lines().map(slint::SharedString::from).collect();
            let (expect, _) = crate::panel_ctl::wrap_lines(
                &lines,
                usize::try_from(f.h.comp.get_panel_wrap_cols()).unwrap(),
                &|| false,
            );
            assert_eq!(rows_in_panel(&f), expect.len(), "scale {scale}: exact");
        }
    }

    #[test]
    fn a_small_file_wrap_per_step_is_cheap() {
        // The common case: a whole drag step (model build, push, top-line restore) on a file just under the live
        // threshold. Judged against one bare wrap of the same file, so a loaded machine moves both.
        let (f, _h, _) = live(Opts::default());
        let body: String = (0..1000)
            .map(|i| {
                format!(
                    "line {i} {}
",
                    "word ".repeat(34).trim_end()
                )
            })
            .collect();
        assert!((170_000..=200_000).contains(&body.len()), "{}", body.len());
        show_text(&f, "small.txt", &body);
        f.pump_until("the wrapped rows", |f| {
            f.h.comp.get_panel_kind() == 2 && f.h.comp.get_panel_wrapped()
        });
        let lines: Vec<slint::SharedString> = body.lines().map(slint::SharedString::from).collect();
        let t = Instant::now();
        let _ = crate::panel_ctl::wrap_lines(&lines, 40, &|| false);
        let base = t.elapsed();
        let x = f.h.comp.get_panel_splitter_x() + f.h.comp.get_panel_splitter_w() / 2.0;
        let y = f.h.comp.get_grid_top() + 200.0;
        f.h.move_to(x, y);
        f.h.press(x, y, slint::platform::PointerEventButton::Left);
        let mut steps = Vec::new();
        for k in 1..=24 {
            let t = Instant::now();
            drag_to(&f, x, y, -7.0 * k as f32);
            steps.push(t.elapsed());
        }
        f.h.release(x, y, slint::platform::PointerEventButton::Left);
        steps.sort();
        let median = steps[steps.len() / 2];
        eprintln!("small-file step {median:?} (bare wrap {base:?})");
        assert!(
            median < base * 40 + Duration::from_millis(60),
            "a step took {median:?}, a bare wrap {base:?}"
        );
    }

    /// A file between the live threshold and the settle threshold (about 540 KB).
    fn mid_body() -> String {
        format!(
            "{}
",
            "word ".repeat(120).trim_end()
        )
        .repeat(900)
    }

    #[test]
    fn a_window_edge_drag_never_wraps_a_mid_file_inline_nor_restarts_the_worker() {
        let (f, _h, _) = live(Opts::default());
        let body = mid_body();
        assert!(body.len() > 200_000 && body.len() < 1_000_000);
        show_text(&f, "mid.txt", &body);
        f.pump_until("the wrapped rows", |f| {
            f.h.comp.get_panel_kind() == 2 && f.h.comp.get_panel_wrapped()
        });
        let rows0 = rows_in_panel(&f);
        let runs0 = f.ctl.borrow().panels.wrap_count();
        // The slot's width moves with no splitter held (what a window edge does), and a refresh comes between steps
        // (a shell redraw, an agent streaming).
        f.h.comp.set_panel_share(0.3);
        f.h.advance(0);
        assert_eq!(
            rows_in_panel(&f),
            rows0,
            "the first step did not wrap inline"
        );
        let runs1 = f.ctl.borrow().panels.wrap_count();
        assert_eq!(runs1, runs0 + 1, "one worker wrap started");
        for share in [0.33f32, 0.36, 0.4] {
            f.h.comp.set_panel_share(share);
            f.h.advance(0);
            f.ctl.borrow_mut().refresh();
            assert_eq!(
                f.ctl.borrow().panels.wrap_count(),
                runs1,
                "a step or a refresh with a wrap in flight starts nothing (no inline wrap, no restart)"
            );
            assert_eq!(rows_in_panel(&f), rows0, "still no inline wrap");
        }
        // The worker's answer arrives before any settle timer.
        f.pump_until("a worker answer", |f| rows_in_panel(f) != rows0);
        f.ctl.borrow_mut().panels.set_wrap_moving(false);
        f.ctl.borrow_mut().refresh();
        f.pump_until("the exact wrap", |f| {
            f.h.advance(0);
            f.h.comp.get_panel_wrapped_cols() == f.h.comp.get_panel_wrap_cols()
        });
    }

    /// A file of 300 long lines, each wrapping to several rows.
    fn many_long_lines() -> String {
        (0..300)
            .map(|i| format!("line {i} {}\n", "word ".repeat(40).trim_end()))
            .collect()
    }

    /// The number of the line at the top of the text view (the nearest numbered row at or above the first).
    fn top_line_number(f: &Fx) -> i32 {
        let nums = nums_in_panel(f);
        let first = usize::try_from(f.h.comp.get_panel_first_line()).unwrap();
        if nums.is_empty() {
            return i32::try_from(first).unwrap() + 1;
        }
        (0..=first.min(nums.len() - 1))
            .rev()
            .map(|r| nums[r])
            .find(|n| *n > 0)
            .unwrap_or(1)
    }

    #[test]
    fn a_rewrap_keeps_the_line_the_reader_was_on_at_the_top() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        show_text(&f, "many.txt", &many_long_lines());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        // Down to somewhere in the middle of the file.
        f.h.comp.invoke_panel_scroll_by(0.0, -18.0 * 700.0);
        f.h.advance(0);
        let line = top_line_number(&f);
        assert!(line > 100, "scrolled far into the file (line {line})");
        let rows0 = rows_in_panel(&f);
        // A much wider slot: fewer rows, other row offsets.
        f.h.comp.invoke_panel_splitter_released(0.6);
        f.pump();
        f.h.advance(0);
        f.h.advance(0);
        assert!(rows_in_panel(&f) < rows0, "the rows were made again");
        assert_eq!(
            top_line_number(&f),
            line,
            "the same line is still at the top after the re-wrap"
        );
        // And a narrower slot back: the same.
        f.h.comp.invoke_panel_splitter_released(0.25);
        f.pump();
        f.h.advance(0);
        f.h.advance(0);
        assert_eq!(top_line_number(&f), line, "and back again");
        // Toggling wrap off keeps it too (the rows are the lines: row = line - 1).
        f.h.comp.invoke_panel_wrap_toggled();
        f.pump();
        f.h.advance(0);
        f.h.advance(0);
        assert_eq!(top_line_number(&f), line, "wrap off");
        f.h.comp.invoke_panel_wrap_toggled();
        f.pump();
        f.h.advance(0);
        f.h.advance(0);
        assert_eq!(top_line_number(&f), line, "wrap on");
        // The view never sits past the end: the last row is still reachable.
        let _ = a;
        assert!(
            f.h.comp.get_panel_text_scroll_y() <= 0.0,
            "the viewport is inside the content"
        );
    }

    #[test]
    fn turning_wrap_on_brings_a_sideways_scrolled_view_back_to_the_left_edge() {
        let (f, _h, _) = live(Opts::default());
        flare_settings::term_prefs::set_wrap_previews(&f.prefs, false);
        f.pump();
        show_text(&f, "long.txt", &long_body());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        assert!(!f.h.comp.get_panel_wrapped());
        f.h.comp.invoke_panel_scroll_by(-200.0, 0.0);
        f.h.advance(0);
        assert!(
            f.h.comp.get_panel_text_scroll_x() < -50.0,
            "scrolled right ({})",
            f.h.comp.get_panel_text_scroll_x()
        );
        f.h.comp.invoke_panel_wrap_toggled();
        f.pump();
        f.h.advance(0);
        f.h.advance(0);
        assert!(f.h.comp.get_panel_wrapped());
        assert_eq!(
            f.h.comp.get_panel_text_scroll_x(),
            0.0,
            "wrapped rows start at the gutter's edge again"
        );
    }

    #[test]
    fn settings_in_front_wraps_nothing_and_the_return_wraps_once() {
        let (f, _h, _) = live(Opts::default());
        show_text(&f, "long.txt", &long_body());
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        let rows = rows_in_panel(&f);
        let runs = f.ctl.borrow().panels.wrap_count();
        f.ctl.borrow_mut().toggle_settings();
        f.pump();
        f.h.advance(0);
        assert_eq!(
            f.ctl.borrow().panels.wrap_count(),
            runs,
            "no wrap while a page is in front (the slot is off show)"
        );
        f.ctl.borrow_mut().toggle_settings();
        f.pump_until("the text again", |f| f.h.comp.get_panel_kind() == 2);
        f.h.advance(0);
        assert_eq!(rows_in_panel(&f), rows);
        assert!(f.h.comp.get_panel_wrapped());
        assert_eq!(
            f.ctl.borrow().panels.wrap_count(),
            runs + 1,
            "the return wraps the file once, not an unwrap and a wrap"
        );
    }

    #[test]
    fn a_refused_show_in_side_panel_tells_the_person_in_the_panel() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        assert!(!f.h.comp.get_panel_open());
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Term(TermMsgBox(TermMsg::PreviewRefused { tab: id })));
        f.pump_until("the notice", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "Cannot show this file");
        assert!(f.openers.records().is_empty());
    }

    #[test]
    fn a_click_on_a_file_in_a_window_too_narrow_for_a_slot_opens_it_in_windows() {
        let (f, _h, _) = live(Opts::default());
        let abs = f.dir.path().join("narrow.txt");
        std::fs::write(&abs, "x").unwrap(); // sync-ok: test code only
        let abs = abs.to_string_lossy().into_owned();
        // No room for a slot (the same as a half-screen window with vertical tabs).
        f.h.comp.set_panel_usable_min(10_000.0);
        f.h.advance(0);
        assert!(!f.h.comp.get_panel_fits());
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl
            .borrow_mut()
            .show_preview(&id, flare_surfaces::PanelKind::Text, &abs);
        let t = Instant::now();
        while f.openers.records().is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: test code only
        }
        assert_eq!(f.openers.records().len(), 1, "the click did something");
        assert!(!f.h.comp.get_panel_open(), "and opened no invisible panel");
    }

    #[test]
    fn a_tab_that_comes_back_finds_its_file_where_the_reader_left_it() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        let body: String = (1..=300).map(|i| format!("line {i}\n")).collect();
        show_text(&f, "long.txt", &body);
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        f.h.advance(0);
        f.h.comp.invoke_panel_scroll_to_row(120);
        f.h.advance(0);
        let first = f.h.comp.get_panel_first_line();
        assert!(first >= 100, "scrolled: {first}");
        // Another tab with a panel of its own, then back.
        let _b = f.ctl.borrow_mut().open_folder(r"C:\b");
        f.ctl.borrow_mut().refresh();
        show_text(&f, "other.txt", "x");
        f.pump_until("the other file", |f| {
            f.h.comp.get_panel_title().as_str() == "other.txt"
        });
        f.ctl.borrow_mut().model_mut().pick(&a);
        f.ctl.borrow_mut().refresh();
        f.pump_until("the file back", |f| {
            f.h.comp.get_panel_title().as_str() == "long.txt" && rows_in_panel(f) == 300
        });
        f.h.advance(0);
        let back = f.h.comp.get_panel_first_line();
        assert!(
            (back - first).abs() <= 1,
            "came back at line {back}, left at {first}"
        );
    }

    #[test]
    fn a_refused_show_in_side_panel_leaves_the_panel_that_is_open_as_it_was() {
        let (f, _h, _) = live(Opts::default());
        show_text(&f, "keep.txt", "one\ntwo");
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
        let id = f.ctl.borrow().front_id().unwrap();
        let rows = rows_in_panel(&f);
        let target = f.ctl.borrow().panels.snap(&id).unwrap().path;
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Term(TermMsgBox(TermMsg::PreviewRefused {
                tab: id.clone(),
            })));
        f.pump();
        f.h.advance(0);
        assert_eq!(f.h.comp.get_panel_kind(), 2, "still the text file");
        assert_eq!(rows_in_panel(&f), rows);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "keep.txt");
        assert!(
            f.h.comp.get_panel_sub().as_str().starts_with("Cannot show"),
            "{}",
            f.h.comp.get_panel_sub()
        );
        assert_eq!(f.ctl.borrow().panels.snap(&id).unwrap().path, target);
        // Showing something else ends the note.
        show_text(&f, "other.txt", "x");
        f.pump_until("the other file", |f| {
            f.h.comp.get_panel_title().as_str() == "other.txt"
        });
        assert!(!f.h.comp.get_panel_sub().as_str().starts_with("Cannot show"));
    }

    #[test]
    fn a_big_file_wraps_on_a_worker_and_a_stale_answer_changes_nothing() {
        use crate::panel_ctl::{PanelMsg, PanelMsgBox};
        let (f, _h, _) = live(Opts::default());
        let body = format!("{}\n", "x".repeat(1500)).repeat(900);
        show_text(&f, "big.txt", &body);
        f.pump_until("the wrapped rows", |f| {
            f.h.comp.get_panel_kind() == 2 && f.h.comp.get_panel_wrapped()
        });
        let rows = rows_in_panel(&f);
        assert!(rows > 900, "{rows}");
        // An answer for a wrap that is no longer the newest is dropped.
        let id = f.ctl.borrow().front_id().unwrap();
        let generation = f.ctl.borrow().panels.generation_for_test();
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Panel(PanelMsgBox(PanelMsg::Wrapped {
                generation,
                tab: id,
                seq: 0,
                cols: 9,
                rows: vec!["stale".into()],
                nums: vec![1],
            })));
        assert_eq!(rows_in_panel(&f), rows);
    }

    #[test]
    fn the_splitter_release_stores_the_share_for_that_tab_only() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        show_text(&f, "a.txt", "a");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(
            f.h.comp.get_panel_share(),
            0.0,
            "not set until the splitter moves"
        );
        f.h.comp.invoke_panel_splitter_released(0.31);
        assert!((f.h.comp.get_panel_share() - 0.31).abs() < 1e-6);
        // Another tab with a panel starts at the default.
        let _b = f.ctl.borrow_mut().open_folder("C:\\b");
        show_text(&f, "b.txt", "b");
        f.pump_until("the second panel", |f| {
            f.h.comp.get_panel_title().as_str() == "b.txt" && f.h.comp.get_panel_kind() == 2
        });
        assert_eq!(f.h.comp.get_panel_share(), 0.0);
        f.ctl.borrow_mut().model_mut().pick(&a);
        f.ctl.borrow_mut().refresh();
        assert!(
            (f.h.comp.get_panel_share() - 0.31).abs() < 1e-6,
            "the first tab kept its own"
        );
        // A nonsense share is refused.
        f.h.comp.invoke_panel_splitter_released(7.0);
        assert!((f.h.comp.get_panel_share() - 0.31).abs() < 1e-6);
    }

    #[test]
    fn an_answer_for_a_file_no_longer_on_show_is_dropped() {
        use crate::panel_ctl::{PanelMsg, PanelMsgBox};
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        show_text(&f, "a.txt", "real");
        f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
        let stale = || crate::panel_ctl::TextRows {
            lines: Arc::new(vec!["stale".into(), "stale".into()]),
            cols: 5,
            bytes: 12,
            foot: String::new(),
        };
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Panel(PanelMsgBox(PanelMsg::Text {
                generation: 12345,
                tab: id.clone(),
                result: Ok(stale()),
            })));
        assert_eq!(rows_in_panel(&f), 1, "the old generation changed nothing");
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Panel(PanelMsgBox(PanelMsg::Text {
                generation: 1,
                tab: "t-other".into(),
                result: Ok(stale()),
            })));
        assert_eq!(rows_in_panel(&f), 1, "another tab's answer changed nothing");
    }

    #[test]
    fn a_new_click_replaces_the_panel_and_a_late_answer_of_the_old_one_is_dropped() {
        let (f, _h, _) = live(Opts::default());
        show_text(&f, "one.txt", "one");
        show_text(&f, "two.txt", "two\ntwo");
        f.pump_until("the second file", |f| {
            f.h.comp.get_panel_title().as_str() == "two.txt" && f.h.comp.get_panel_kind() == 2
        });
        // Both workers have answered by now; the first's answer, whenever it came, did not win.
        f.pump();
        assert_eq!(f.h.comp.get_panel_title().as_str(), "two.txt");
        assert_eq!(rows_in_panel(&f), 2);
    }

    #[test]
    fn a_panel_comes_back_with_its_tab_and_a_closed_one_stays_closed() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", None)],
            ..Opts::default()
        });
        let note = f.dir.path().join("note.txt");
        std::fs::write(&note, "kept\nlines").unwrap(); // sync-ok: test code only
        let target = note.to_string_lossy().into_owned();
        let panel = |open: bool, width: Option<f32>| flare_surfaces::TabPanel {
            kind: flare_surfaces::PanelKind::Text,
            target: target.clone(),
            open,
            width,
            by_agent: false,
            view: None,
        };
        let file = f.dir.path().join("panels.json");
        let saved = [
            flare_surfaces::store::SavedTabPanel {
                cwd: "C:\\a".into(),
                panel: Some(panel(true, Some(0.3))),
            },
            flare_surfaces::store::SavedTabPanel {
                cwd: "C:\\b".into(),
                panel: Some(panel(false, None)),
            },
        ];
        std::fs::write(&file, flare_surfaces::store::panels_json(&saved)).unwrap(); // sync-ok: test code only
        let store = Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        ));
        f.ctl.borrow_mut().set_panel_store(store.clone());
        f.start_restore_and_wait();
        // The strip restores with the first tab in front: its panel is open at its width.
        f.pump_until("the restored panel", |f| f.h.comp.get_panel_kind() == 2);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "note.txt");
        assert!((f.h.comp.get_panel_share() - 0.3).abs() < 1e-6);
        assert_eq!(rows_in_panel(&f), 2);
        // The second tab's panel was closed when saved: it is still closed.
        assert_eq!(f.ctl.borrow().open_panels(), 1);
        // A change is written: close the first, and the file says so.
        f.ctl.borrow_mut().close_front_panel();
        store.flush();
        let back = flare_surfaces::store::PanelStore::new(&file, Duration::from_millis(30)).load();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].panel.as_ref().map(|p| p.open), Some(false));
        assert_eq!(back[0].panel.as_ref().and_then(|p| p.width), Some(0.3));
    }

    #[test]
    fn a_stored_panel_whose_target_is_not_a_local_file_of_its_kind_is_dropped() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            ..Opts::default()
        });
        let file = f.dir.path().join("panels.json");
        // A network path (it would make Windows reach out), and a script named as a picture's kind.
        for target in [r"\\server\share\p.png", r"C:\x\run.bat", "relative.png"] {
            let raw = format!(
                r#"{{"version":1,"tabs":[{{"cwd":"C:\\a","panel":{{"kind":"image","target":{}}}}}]}}"#,
                serde_json::Value::String(target.into())
            );
            std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
            let g = fx_with(Opts {
                saved: vec![("C:\\a", None)],
                ..Opts::default()
            });
            let gfile = g.dir.path().join("panels.json");
            std::fs::copy(&file, &gfile).unwrap(); // sync-ok: test code only
            g.ctl
                .borrow_mut()
                .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
                    &gfile,
                    Duration::from_millis(30),
                )));
            g.start_restore_and_wait();
            assert_eq!(g.ctl.borrow().open_panels(), 0, "{target}");
            assert!(!g.h.comp.get_panel_open(), "{target}");
        }
    }

    // ---- the control channel's calls (Surfaces stage 2) ----

    /// One control channel op, answered the way the worker would get it.
    fn ctl_call(
        f: &Fx,
        op: crate::ctl_app::CtlOp,
    ) -> Result<serde_json::Value, flare_ctl::CtlError> {
        let (tx, rx) = std::sync::mpsc::channel();
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
                crate::ctl_app::CtlRequest {
                    op,
                    deadline: std::time::Instant::now() + Duration::from_secs(5),
                    reply: tx,
                },
            )));
        rx.try_recv().expect("the UI thread answers at once")
    }

    fn show_text_op(tab: &str, text: &str, title: Option<&str>) -> crate::ctl_app::CtlOp {
        crate::ctl_app::CtlOp::ShowText {
            tab: tab.into(),
            text: text.into(),
            title: title.map(str::to_string),
        }
    }

    type Events = Arc<std::sync::Mutex<Vec<(String, serde_json::Value, Option<String>)>>>;

    /// Starts recording the control channel events the controller publishes.
    fn recorded_events(f: &Fx) -> Events {
        let store: Events = Arc::default();
        let keep = Arc::clone(&store);
        f.ctl.borrow_mut().set_ctl(
            Arc::new(move |name, params, tab| {
                keep.lock()
                    .unwrap()
                    .push((name.to_string(), params, tab.map(str::to_string)));
            }),
            crate::appctl::CtlInfo::default(),
        );
        store
    }

    #[test]
    fn view_text_shows_in_the_front_tabs_panel_with_its_title() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        let v = ctl_call(&f, show_text_op(&id, "alpha\nbeta\ngamma", Some("Plan"))).unwrap();
        assert_eq!(v["panel"]["kind"], "text");
        assert_eq!(v["panel"]["title"], "Plan");
        assert!(v["panel"].get("path").is_none(), "text has no file");
        f.pump_until("the text", |f| rows_in_panel(f) == 3);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "Plan");
        assert_eq!(f.h.comp.get_panel_kind(), 2);
        assert!(f.h.comp.get_panel_open());
        assert!(
            f.h.comp.get_panel_sub().as_str().starts_with("3 lines"),
            "{}",
            f.h.comp.get_panel_sub()
        );
        // A second call replaces it; with no title the header says Text.
        ctl_call(&f, show_text_op(&id, "one line", None)).unwrap();
        f.pump_until("the new text", |f| rows_in_panel(f) == 1);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "Text");
        // Text that was handed over never reaches the disk's panel file, and asks nothing of a path.
        assert_eq!(f.ctl.borrow().open_panels(), 1);
    }

    #[test]
    fn view_file_shows_a_file_under_a_title_and_a_picture_with_its_size() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        let note = f.dir.path().join("n.txt");
        std::fs::write(&note, "x\ny").unwrap(); // sync-ok: test code only
        let abs = note.to_string_lossy().into_owned();
        let v = ctl_call(
            &f,
            crate::ctl_app::CtlOp::ShowFile {
                tab: id.clone(),
                kind: flare_surfaces::PanelKind::Text,
                abs: abs.clone(),
                title: Some("Notes".into()),
            },
        )
        .unwrap();
        assert_eq!(v["panel"]["kind"], "text");
        assert_eq!(v["panel"]["path"], abs.as_str());
        f.pump_until("the file", |f| rows_in_panel(f) == 2);
        assert_eq!(f.h.comp.get_panel_title().as_str(), "Notes");
        // The same file without a title is shown under its own name again.
        ctl_call(
            &f,
            crate::ctl_app::CtlOp::ShowFile {
                tab: id,
                kind: flare_surfaces::PanelKind::Text,
                abs,
                title: None,
            },
        )
        .unwrap();
        f.pump_until("the name", |f| {
            f.h.comp.get_panel_title().as_str() == "n.txt"
        });
    }

    #[test]
    fn a_call_for_a_tab_that_is_behind_stores_its_panel_and_never_switches_tabs() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        assert_eq!(f.ctl.borrow().front_id().as_deref(), Some(b.as_str()));
        ctl_call(&f, show_text_op(&a, "for a", Some("A"))).unwrap();
        f.pump();
        assert_eq!(
            f.ctl.borrow().front_id().as_deref(),
            Some(b.as_str()),
            "the front tab did not change"
        );
        assert!(!f.h.comp.get_panel_open(), "b has no panel");
        assert_eq!(f.ctl.borrow().open_panels(), 1);
        // a's panel is there when a comes to the front.
        f.ctl.borrow_mut().model_mut().pick(&a);
        f.ctl.borrow_mut().refresh();
        f.pump_until("a's panel", |f| f.h.comp.get_panel_title().as_str() == "A");
    }

    #[test]
    fn panel_state_and_close_report_what_is_open() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        let state =
            |f: &Fx| ctl_call(f, crate::ctl_app::CtlOp::PanelState { tab: id.clone() }).unwrap();
        assert_eq!(state(&f), serde_json::json!({"open": false}));
        let close = |f: &Fx| {
            ctl_call(f, crate::ctl_app::CtlOp::PanelClose { tab: id.clone() }).unwrap()["closed"]
                .clone()
        };
        assert_eq!(close(&f), false, "nothing open is not an error");
        ctl_call(&f, show_text_op(&id, "x", Some("T"))).unwrap();
        let s = state(&f);
        assert_eq!(s["open"], true);
        assert_eq!(s["panel"]["kind"], "text");
        assert_eq!(s["panel"]["title"], "T");
        assert_eq!(
            s["width"],
            serde_json::json!(flare_surfaces::layout::DEFAULT_SHARE)
        );
        assert_eq!(close(&f), true);
        f.pump();
        assert!(!f.h.comp.get_panel_open());
        assert_eq!(state(&f), serde_json::json!({"open": false}));
        assert_eq!(close(&f), false);
        // The toggle chord brings back what the agent showed.
        assert!(f.ctl.borrow_mut().toggle_front_panel());
        assert_eq!(state(&f)["open"], true);
    }

    #[test]
    fn the_tab_list_has_every_shell_tab_or_only_the_callers() {
        let (f, _h, _) = live(Opts::default());
        let a = f.ctl.borrow().front_id().unwrap();
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        let all = ctl_call(&f, crate::ctl_app::CtlOp::Tabs { own: None }).unwrap();
        let ids: Vec<&str> = all["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [a.as_str(), b.as_str()]);
        assert_eq!(all["tabs"][1]["active"], true);
        assert_eq!(all["tabs"][0]["active"], false);
        assert!(!all["tabs"][0]["title"].as_str().unwrap().is_empty());
        let own = ctl_call(
            &f,
            crate::ctl_app::CtlOp::Tabs {
                own: Some(a.clone()),
            },
        )
        .unwrap();
        assert_eq!(own["tabs"].as_array().unwrap().len(), 1);
        assert_eq!(own["tabs"][0]["id"], a.as_str());
        let cwd = ctl_call(&f, crate::ctl_app::CtlOp::TabCwd { tab: a }).unwrap();
        assert_eq!(cwd["cwd"], "C:\\a");
    }

    #[test]
    fn a_closed_or_invented_tab_is_no_tab() {
        let (f, _h, _) = live(Opts::default());
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        f.ctl.borrow_mut().request_close(&b);
        f.pump();
        for tab in [b.as_str(), "nope", "settings", ""] {
            for op in [
                show_text_op(tab, "x", None),
                crate::ctl_app::CtlOp::PanelState { tab: tab.into() },
                crate::ctl_app::CtlOp::PanelClose { tab: tab.into() },
                crate::ctl_app::CtlOp::TabCwd { tab: tab.into() },
            ] {
                let e = ctl_call(&f, op).unwrap_err();
                assert_eq!(e.code, flare_ctl::code::NO_TAB, "{tab:?}");
            }
        }
        assert_eq!(f.ctl.borrow().open_panels(), 0);
    }

    #[test]
    fn an_explicit_ask_shows_even_when_clicks_open_previews_in_windows() {
        let (f, _h, _) = live(Opts::default());
        f.prefs.set("flare.window.openPreviews", "0");
        f.pump();
        let id = f.ctl.borrow().front_id().unwrap();
        ctl_call(&f, show_text_op(&id, "still shown", None)).unwrap();
        f.pump_until("the text", |f| rows_in_panel(f) == 1);
    }

    #[test]
    fn text_handed_over_is_not_written_to_the_panels_file_and_not_restored() {
        let (f, _h, _) = live(Opts::default());
        let file = f.dir.path().join("panels.json");
        let store = Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        ));
        f.ctl.borrow_mut().set_panel_store(store.clone());
        // Tab a holds handed-over text; tab b holds a real file, so the store has something to write
        // and a missing file cannot pass for "nothing was saved".
        let a = f.ctl.borrow().front_id().unwrap();
        let b = f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        let note = f.dir.path().join("kept.txt");
        std::fs::write(&note, "kept").unwrap(); // sync-ok: test code only
        ctl_call(&f, show_text_op(&a, "private notes", Some("T"))).unwrap();
        ctl_call(
            &f,
            crate::ctl_app::CtlOp::ShowFile {
                tab: b.clone(),
                kind: flare_surfaces::PanelKind::Text,
                abs: note.to_string_lossy().into_owned(),
                title: None,
            },
        )
        .unwrap();
        f.ctl.borrow_mut().refresh();
        store.flush();
        let raw = std::fs::read_to_string(&file).expect("the store wrote the file"); // sync-ok: test code only
        assert!(raw.contains("kept.txt"), "the file panel is saved: {raw}");
        assert!(!raw.contains("private notes"), "{raw}");
        assert!(!raw.contains("inline"), "{raw}");
        let back = flare_surfaces::store::parse_panels(&raw);
        assert_eq!(back.len(), 2);
        assert!(back[0].panel.is_none(), "the text tab saves no panel");
        assert!(back[1].panel.is_some());
        // A new run restores from that file: tab b gets its file, tab a gets nothing.
        let g = fx_with(Opts {
            saved: vec![("C:\\a", None), ("C:\\b", None)],
            ..Opts::default()
        });
        let gfile = g.dir.path().join("panels.json");
        std::fs::copy(&file, &gfile).unwrap(); // sync-ok: test code only
        g.ctl
            .borrow_mut()
            .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
                &gfile,
                Duration::from_millis(30),
            )));
        g.start_restore_and_wait();
        assert_eq!(g.ctl.borrow().open_panels(), 1, "only the file came back");
        assert!(
            !g.h.comp.get_panel_open(),
            "the text tab (in front) has no panel"
        );
    }

    #[test]
    fn a_click_after_a_titled_call_shows_the_file_under_its_own_name() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        ctl_call(&f, show_text_op(&id, "x", Some("Plan"))).unwrap();
        f.pump_until("the title", |f| {
            f.h.comp.get_panel_title().as_str() == "Plan"
        });
        let events = recorded_events(&f);
        let note = f.dir.path().join("clicked.txt");
        std::fs::write(&note, "c").unwrap(); // sync-ok: test code only
        f.ctl.borrow_mut().show_preview(
            &id,
            flare_surfaces::PanelKind::Text,
            &note.to_string_lossy(),
        );
        f.pump_until("the file's name", |f| {
            f.h.comp.get_panel_title().as_str() == "clicked.txt"
        });
        let state = ctl_call(&f, crate::ctl_app::CtlOp::PanelState { tab: id.clone() }).unwrap();
        assert_eq!(state["panel"]["title"], "clicked.txt", "{state}");
        let ev = events.lock().unwrap().clone();
        assert_eq!(ev.last().unwrap().1["panel"]["title"], "clicked.txt");
        // A file given a title keeps it only until the next click or call.
        ctl_call(
            &f,
            crate::ctl_app::CtlOp::ShowFile {
                tab: id.clone(),
                kind: flare_surfaces::PanelKind::Text,
                abs: note.to_string_lossy().into_owned(),
                title: Some("Fake".into()),
            },
        )
        .unwrap();
        f.pump_until("the given title", |f| {
            f.h.comp.get_panel_title().as_str() == "Fake"
        });
        f.ctl.borrow_mut().show_preview(
            &id,
            flare_surfaces::PanelKind::Text,
            &note.to_string_lossy(),
        );
        f.pump_until("the name again", |f| {
            f.h.comp.get_panel_title().as_str() == "clicked.txt"
        });
    }

    #[test]
    fn panel_changes_are_published_to_the_tab_s_clients() {
        let (f, _h, _) = live(Opts::default());
        let events = recorded_events(&f);
        let id = f.ctl.borrow().front_id().unwrap();
        ctl_call(&f, show_text_op(&id, "x", Some("T"))).unwrap();
        ctl_call(&f, crate::ctl_app::CtlOp::PanelClose { tab: id.clone() }).unwrap();
        // The close button and a click on a path say so too.
        let note = f.dir.path().join("p.txt");
        std::fs::write(&note, "p").unwrap(); // sync-ok: test code only
        f.ctl.borrow_mut().show_preview(
            &id,
            flare_surfaces::PanelKind::Text,
            &note.to_string_lossy(),
        );
        f.ctl.borrow_mut().close_front_panel();
        let ev = events.lock().unwrap().clone();
        let opens: Vec<bool> = ev
            .iter()
            .map(|(n, p, t)| {
                assert_eq!(n, "panel.changed");
                assert_eq!(t.as_deref(), Some(id.as_str()));
                assert_eq!(p["tab"], id.as_str());
                p["open"].as_bool().unwrap()
            })
            .collect();
        assert_eq!(opens, [true, false, true, false]);
        assert_eq!(ev[0].1["panel"]["title"], "T");
        assert!(ev[1].1.get("panel").is_none());
    }

    #[test]
    fn a_request_that_arrives_after_its_deadline_changes_nothing() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let past = std::time::Instant::now() - Duration::from_secs(1);
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
                crate::ctl_app::CtlRequest {
                    op: show_text_op(&id, "too late", Some("Late")),
                    deadline: past,
                    reply: tx,
                },
            )));
        let answer = rx.try_recv().expect("answered at once");
        assert_eq!(answer.unwrap_err().code, flare_ctl::code::TIMEOUT);
        f.pump();
        assert_eq!(f.ctl.borrow().open_panels(), 0, "no panel was shown");
        assert!(!f.h.comp.get_panel_open());
    }

    #[test]
    fn a_message_after_the_end_of_life_is_dropped_so_its_waiter_lets_go() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        f.ctl.borrow_mut().end();
        let (tx, rx) = std::sync::mpsc::channel();
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
                crate::ctl_app::CtlRequest {
                    op: show_text_op(&id, "late", None),
                    deadline: std::time::Instant::now() + Duration::from_secs(5),
                    reply: tx,
                },
            )));
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn a_panel_of_a_kind_this_stage_cannot_draw_is_kept_in_the_file_and_not_shown() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", None)],
            ..Opts::default()
        });
        let file = f.dir.path().join("panels.json");
        std::fs::write(
            &file,
            r#"{"version":1,"tabs":[{"cwd":"C:\\a","panel":{"kind":"mod:notes","target":"x"}}]}"#,
        )
        .unwrap(); // sync-ok: test code only
        let store = Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        ));
        f.ctl.borrow_mut().set_panel_store(store.clone());
        f.start_restore_and_wait();
        f.pump();
        assert!(!f.h.comp.get_panel_open(), "no viewer for it yet");
        store.flush();
        let back = flare_surfaces::store::PanelStore::new(&file, Duration::from_millis(30)).load();
        assert_eq!(
            back[0].panel.as_ref().map(|p| p.kind.name()),
            Some("mod:notes".to_string())
        );
    }

    #[test]
    fn a_session_that_never_opens_a_panel_writes_no_file_and_starts_no_writer() {
        let (f, _h, _) = live(Opts::default());
        let file = f.dir.path().join("panels.json");
        let store = Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        ));
        f.ctl.borrow_mut().set_panel_store(store.clone());
        f.ctl.borrow_mut().refresh();
        f.ctl.borrow_mut().open_folder("C:\\b");
        f.pump();
        store.flush();
        assert!(!file.exists());
        assert!(!store.started());
    }

    #[test]
    fn closing_a_tab_tells_term_ui() {
        let (f, _h, _) = live(Opts::default());
        let id = f.ctl.borrow().front_id().unwrap();
        let inputs = f.ctl.borrow().decor_inputs();
        {
            // The tab shows a path in a folder that exists, so it teaches that folder.
            let mut i = inputs.borrow_mut();
            let hit = |p: &str| {
                Some(flare_term::path_open::PathHit {
                    kind: flare_term::path_open::PathKind::File,
                    abs: p.to_string(),
                })
            };
            i.paths
                .link_ranges("see C:\\d\\f.txt", Some("C:\\cwd"), &id, 0);
            i.paths.advance(50);
            let ask = i.paths.take_ask().expect("an ask");
            let hits = ask.paths.iter().map(|p| hit(p)).collect();
            i.paths.answer(&ask, Some(hits), 50);
            i.paths
                .link_ranges("see C:\\d\\f.txt and rel\\x.txt", Some("C:\\cwd"), &id, 60);
        }
        assert_eq!(inputs.borrow().paths.owners_kept(), 1);
        f.ctl.borrow_mut().request_close(&id);
        f.pump();
        assert_eq!(inputs.borrow().paths.owners_kept(), 0);
    }

    // ---- the marks on the rows, the mark clock, reduced motion and minimise (Task 29) -------------

    const OSC_WORKING: &str = "\x1b]777;flare-agent;state=working\x07";
    const OSC_QUESTION: &str = "\x1b]777;flare-agent;state=question\x07";
    const OSC_DONE: &str = "\x1b]777;flare-agent;state=done\x07";
    const OSC_FAILED: &str = "\x1b]777;flare-agent;state=failed;kind=rate_limit\x07";

    impl Fx {
        /// Opens `paths` as tabs (the last in front), all away from the user's eyes.
        fn open_tabs(&self, paths: &[&str]) -> Vec<String> {
            let mut ids = Vec::new();
            for p in paths {
                ids.push(self.ctl.borrow_mut().open_folder(p));
                self.wait_made(p);
            }
            self.ctl.borrow_mut().hub_mut().set_looking(None, false);
            ids
        }

        /// What the shell of `cwd` prints (an agent's hook signal, say).
        fn say(&self, cwd: &str, text: &str) {
            self.made_with(cwd)[0].feed(text.as_bytes());
        }

        fn row(&self, i: usize) -> TabRow {
            self.h.comp.get_tab_rows().row_data(i).expect("a row")
        }

        fn motion(&self) -> (bool, f32, f32, f32) {
            let g = self.h.comp.global::<MarkMotion>();
            (g.get_still(), g.get_spin(), g.get_breathe(), g.get_pulse())
        }

        fn clock_running(&self) -> bool {
            self.ctl.borrow().mark_clock_running()
        }
    }

    fn badge_op(tab: &str, key: &str, icon: &str, tip: &str) -> crate::ctl_app::CtlOp {
        crate::ctl_app::CtlOp::BadgeSet {
            tab: tab.into(),
            source: DecorSource::Agent,
            key: key.into(),
            icon: icon.into(),
            tip: tip.into(),
            tone: flare_tabs::decor::Tone::Ink,
            generation: 0,
        }
    }

    fn progress_op(tab: &str, value: Option<f32>) -> crate::ctl_app::CtlOp {
        crate::ctl_app::CtlOp::ProgressSet {
            tab: tab.into(),
            source: DecorSource::Agent,
            key: "progress".into(),
            value,
            label: "Review".into(),
            generation: 0,
        }
    }

    const TITLE_CLAUDE: &str = "\u{1b}]0;\u{2733} Claude Code\u{7}";
    const SID_A: &str = "b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9";
    const SID_B: &str = "11111111-2222-4333-8444-555555555555";

    fn osc_session(sid: &str) -> String {
        format!("\u{1b}]777;flare-agent;state=working;session={sid}\u{7}")
    }

    /// Plugins phase 1: a badge and a progress go from the channel's op to the drawn row (flat fields and the
    /// tooltip), and only the tab they were put on.
    #[test]
    fn a_badge_and_a_progress_from_the_channel_reach_the_drawn_row() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        let a = ids[0].clone();
        f.claude_in("C:\\a");
        assert_eq!(f.row(0).prog, -1.0);
        assert_eq!(f.row(0).chip_count, 0);
        ctl_call(&f, badge_op(&a, "call-me", "phone", "Call-me is on")).unwrap();
        let r = f.row(0);
        assert_eq!(r.chip_count, 1);
        assert_eq!(
            r.chip1_cmd.as_str(),
            crate::menus_ui::glyph_path(crate::menus_ui::GLYPH_BADGE_PHONE)
        );
        assert!(
            r.tip.as_str().ends_with("\nClaude: Call-me is on"),
            "{}",
            r.tip
        );
        ctl_call(&f, progress_op(&a, Some(0.5))).unwrap();
        let r = f.row(0);
        assert_eq!((r.prog, r.chip_count), (0.5, 1));
        assert!(r.tip.as_str().contains("\nClaude: Review 50%"), "{}", r.tip);
        // The other tab is untouched.
        let b = f.row(1);
        assert_eq!((b.prog, b.chip_count, b.tip.as_str()), (-1.0, 0, "C:\\b"));
        // An unknown amount, then the clears.
        ctl_call(&f, progress_op(&a, None)).unwrap();
        assert_eq!(f.row(0).prog, -2.0);
        let v = ctl_call(
            &f,
            crate::ctl_app::CtlOp::ProgressClear {
                tab: a.clone(),
                source: DecorSource::Agent,
                key: "progress".into(),
            },
        )
        .unwrap();
        assert_eq!(v["cleared"], true);
        ctl_call(
            &f,
            crate::ctl_app::CtlOp::BadgeClear {
                tab: a.clone(),
                source: DecorSource::Agent,
                key: "call-me".into(),
            },
        )
        .unwrap();
        let r = f.row(0);
        assert_eq!((r.prog, r.chip_count, r.tip.as_str()), (-1.0, 0, "C:\\a"));
        assert!(!f.ctl.borrow().has_decor(&a));
        // A tab that is not there is no tab, and a fifth key is refused.
        let e = ctl_call(&f, badge_op("nope", "k", "star", "")).unwrap_err();
        assert_eq!(e.code, flare_ctl::code::NO_TAB);
        for k in ["k1", "k2", "k3", "k4"] {
            ctl_call(&f, badge_op(&a, k, "star", "")).unwrap();
        }
        let e = ctl_call(&f, badge_op(&a, "k5", "star", "")).unwrap_err();
        assert_eq!(e.code, flare_ctl::code::BAD_PARAMS);
        assert!(e.message.contains("at most 4"), "{}", e.message);
    }

    /// The agent's decor belongs to its session: another session id, the agent leaving and the tab closing each
    /// take it away; the first session id after none does not.
    #[test]
    fn the_agent_decor_goes_with_its_session_its_agent_and_its_tab() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        let (a, b) = (ids[0].clone(), ids[1].clone());
        f.say("C:\\a", TITLE_CLAUDE);
        f.pump_until("the agent to be here", |f| f.agent_here(&a));
        ctl_call(&f, badge_op(&a, "call-me", "phone", "")).unwrap();
        f.say("C:\\a", &osc_session(SID_A));
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert_eq!(f.row(0).chip_count, 1, "a first session clears nothing");
        f.say("C:\\a", &osc_session(SID_B));
        f.pump_until("the new session to clear the badge", |f| {
            f.row(0).chip_count == 0
        });
        assert!(!f.ctl.borrow().has_decor(&a));

        // The agent leaves (its title goes): its progress goes.
        ctl_call(&f, progress_op(&a, Some(0.3))).unwrap();
        assert_eq!(f.row(0).prog, 0.3);
        f.say("C:\\a", "\u{1b}]0;\u{7}");
        f.pump_until("the agent to leave", |f| f.row(0).prog == -1.0);

        // The tab closes: its decor goes with it.
        f.claude_in("C:\\b");
        ctl_call(&f, badge_op(&b, "x", "star", "")).unwrap();
        assert!(f.ctl.borrow().has_decor(&b));
        // Closed as a yes to the close ask would (an agent tab asks first).
        f.ctl.borrow_mut().close_now(&b);
        f.pump_until("b to close", |f| !f.ctl.borrow().has_decor(&b));
    }

    /// A plain shell tab holds the agent's token too, but with no agent in it the agent's badge and progress
    /// are refused: nothing would name or clear them. Clearing is harmless and still answers.
    #[test]
    fn a_shell_tab_with_no_agent_gets_no_agent_decor() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a"]);
        let a = ids[0].clone();
        let e = ctl_call(&f, badge_op(&a, "k", "star", "Hi")).unwrap_err();
        assert_eq!(e.code, flare_ctl::code::FORBIDDEN);
        assert!(e.message.contains("no agent"), "{}", e.message);
        let e = ctl_call(&f, progress_op(&a, Some(0.5))).unwrap_err();
        assert_eq!(e.code, flare_ctl::code::FORBIDDEN);
        assert!(!f.ctl.borrow().has_decor(&a));
        assert_eq!((f.row(0).chip_count, f.row(0).prog), (0, -1.0));
        let v = ctl_call(
            &f,
            crate::ctl_app::CtlOp::BadgeClear {
                tab: a.clone(),
                source: DecorSource::Agent,
                key: "k".into(),
            },
        )
        .unwrap();
        assert_eq!(v["cleared"], false);
        // A plugin's decor does not need the tab's agent.
        let op = crate::ctl_app::CtlOp::BadgeSet {
            tab: a.clone(),
            source: DecorSource::Plugin("p".into()),
            key: "k".into(),
            icon: "star".into(),
            tip: String::new(),
            tone: flare_tabs::decor::Tone::Ink,
            generation: 0,
        };
        ctl_call(&f, op).unwrap();
        assert!(f.ctl.borrow().has_decor(&a));
    }

    #[test]
    fn a_hook_signal_marks_the_row_in_place() {
        let f = fx();
        f.open_tabs(&["C:\\a", "C:\\b"]);
        let model = f.h.comp.get_tab_rows();
        let ptr = |m: &ModelRc<TabRow>| m.as_any() as *const dyn std::any::Any as *const ();
        let before = ptr(&model);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("a working row", |f| f.row(0).mark_state == 1);
        assert_eq!(f.row(0).agent, 1, "the indicator knows an agent is here");
        assert_eq!(f.row(0).mark_kind, 1, "Ring is the default");
        assert_eq!(f.row(1).mark_state, 0);
        assert_eq!(
            ptr(&f.h.comp.get_tab_rows()),
            before,
            "the model is never replaced"
        );
        assert_eq!(f.rows(), 2);
    }

    #[test]
    fn the_clock_runs_only_while_something_moves() {
        let f = fx();
        f.open_tabs(&["C:\\a", "C:\\b"]);
        assert!(!f.clock_running(), "idle");
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert!(f.clock_running());
        f.clock.set(100);
        f.ctl.borrow().mark_frame_now();
        let (still, spin_a, ..) = f.motion();
        assert!(!still);
        f.clock.set(400);
        f.ctl.borrow().mark_frame_now();
        let (_, spin_b, ..) = f.motion();
        assert_ne!(spin_a, spin_b, "the spinner turns with the clock");
        // Finished under Ring is a still mark: no clock.
        f.say("C:\\a", OSC_DONE);
        f.pump_until("finished", |f| f.row(0).mark_state == 3);
        assert!(!f.clock_running());
        assert!(f.motion().0, "the marks stand still");
    }

    #[test]
    fn the_clock_is_stopped_while_minimised_and_goes_on_after() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.clock.set(1000);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert!(f.clock_running());
        f.clock.set(1300);
        f.ctl.borrow().mark_frame_now();
        let shown = f.motion().1;
        f.clock.set(1400);
        f.ctl.borrow_mut().on_minimised(true);
        assert!(!f.clock_running());
        assert_eq!(f.motion().1, shown, "values frozen while minimised");
        f.clock.set(9000);
        // Even a frame asked for later reads the paused clock: 400 ms in, not 8000.
        f.ctl.borrow().mark_frame_now();
        assert_eq!(
            f.motion().1,
            flare_agent::mark_motion::mark_phase(400).spin_deg
        );
        f.ctl.borrow_mut().on_minimised(false);
        assert!(f.clock_running());
        // 1400 ms stood for 7600 ms: the clock reads 1400 now, 400 ms after it started at 1000.
        let want = flare_agent::mark_motion::mark_phase(400).spin_deg;
        assert_eq!(f.motion().1, want, "goes on from where it stood");
    }

    /// Parity line 24: only the mark animation pauses while minimised; the agents keep their clocks
    /// (PT: no background throttling, `marksPause.ts` pauses the marks alone).
    #[test]
    fn a_minimised_window_pauses_marks_only() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.clock.set(1000);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert!(f.clock_running());
        f.ctl.borrow_mut().on_minimised(true);
        assert!(f.ctl.borrow().marks_paused());
        assert!(!f.clock_running(), "the mark clock stops while minimised");
    }

    #[test]
    fn the_agents_keep_reporting_while_minimised() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.clock.set(1000);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        f.ctl.borrow_mut().on_minimised(true);
        // The agents are not throttled: a state they report while the window is down still lands.
        f.clock.set(2000);
        f.say("C:\\a", OSC_DONE);
        f.pump_until("finished while minimised", |f| f.row(0).mark_state == 3);
        assert!(
            !f.clock_running(),
            "a finished mark is still, so no clock runs"
        );
    }

    #[test]
    fn animation_effects_off_stills_every_mark() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.ctl.borrow_mut().set_animations_source(Rc::new(|| false));
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert!(!f.clock_running());
        let (still, spin, breathe, pulse) = f.motion();
        assert!(still);
        assert_eq!((spin, breathe, pulse), (0.0, 1.0, 1.0));
        // Back on: the clock runs again.
        f.ctl.borrow_mut().set_animations_source(Rc::new(|| true));
        assert!(f.clock_running());
        assert!(!f.motion().0);
    }

    /// Owner decision 2026-10-26: a plugin's pick moves a working agent's icon, every frame from the mark clock
    /// (`push_frame`); the tab that stops working stands still, and so does every icon with animation effects off.
    #[test]
    fn a_picked_motion_moves_a_working_agents_icon_from_the_mark_clock_and_stills_with_effects_off()
    {
        use flare_agent::icon_motion::{AGY_RGB, IconMotion, icon_phase};
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert_eq!(f.row(0).agent, 1, "Claude, by its hook");
        assert_eq!(f.row(0).motion, 0, "no pick: the icon stands as today");
        f.ctl
            .borrow_mut()
            .motions
            .set(AgentKind::Claude, "p", IconMotion::HopBlink);
        f.ctl.borrow_mut().refresh();
        assert_eq!(f.row(0).motion, IconMotion::HopBlink.code());
        assert!(f.clock_running(), "the working mark already runs the clock");
        let icon = |f: &Fx| {
            let g = f.h.comp.global::<MarkMotion>();
            (g.get_still(), g.get_hop_y(), g.get_blink(), g.get_fade())
        };
        let start = f.clock.get();
        for ms in [0, 513, 810, 1700, 3000] {
            f.clock.set(start + ms);
            f.ctl.borrow().mark_frame_now();
            let p = icon_phase(ms);
            assert_eq!(icon(&f), (false, p.hop_y, p.blink, p.fade), "at {ms}");
        }
        let g = f.h.comp.global::<MarkMotion>();
        assert_eq!(g.get_agy_flow().row_count(), 10);
        // The tab finished: its icon stands still, and with nothing else moving the clock stops.
        f.say("C:\\a", OSC_DONE);
        f.pump_until("finished", |f| f.row(0).mark_state == 3);
        assert_eq!(f.row(0).motion, 0);
        assert!(!f.clock_running());
        // Working again with animation effects off: the pick stays, nothing moves.
        f.ctl.borrow_mut().set_animations_source(Rc::new(|| false));
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert!(!f.clock_running());
        let (still, hop_y, blink, fade) = icon(&f);
        assert!(still && !blink);
        assert_eq!((hop_y, fade), (0.0, 1.0));
        let flow: Vec<slint::Color> = g.get_agy_flow().iter().collect();
        let want: Vec<slint::Color> = AGY_RGB
            .iter()
            .map(|&(r, gr, b)| slint::Color::from_rgb_u8(r, gr, b))
            .collect();
        assert_eq!(flow, want, "the still arch wears its own colours");
    }

    #[test]
    fn a_loading_tab_needs_the_clock_and_a_skeleton_needs_the_pulse() {
        let f = fx_with(Opts {
            saved: vec![("C:\\a", Some(AgentKind::Claude))],
            ..Opts::default()
        });
        assert!(f.row(0).loading);
        assert!(f.clock_running(), "a tab coming back needs the clock");
        assert_eq!(f.motion().3, 1.0, "no skeleton, no pulse");
        f.clock.set(500);
        f.ctl.borrow_mut().set_skeleton_shown(true);
        assert_eq!(f.motion().3, flare_agent::mark_motion::pulse_at(500));
        f.ctl.borrow_mut().set_skeleton_shown(false);
        assert_eq!(f.motion().3, 1.0);
    }

    #[test]
    fn a_palette_change_recolours_the_rows() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        let before = f.row(0).mark_color;
        let store = flare_settings::look::LookStore::new(f.prefs.clone());
        let other = if store.theme_id() == "paper" {
            "pt-default"
        } else {
            "paper"
        };
        store.set_theme_id(other);
        f.pump_until("the new colour", |f| f.row(0).mark_color != before);
    }

    #[test]
    fn the_indicator_pick_turns_a_ring_row_into_a_run_bar() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.row(0).mark_state == 1);
        assert_eq!(f.row(0).mark_kind, 1);
        flare_settings::look::LookStore::new(f.prefs.clone())
            .set_agent_indicator(flare_settings::look::AgentIndicator::Minimal);
        f.pump_until("a run bar", |f| f.row(0).mark_kind == 3);
    }

    #[test]
    fn a_failed_row_carries_the_kind_in_its_tip() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        f.say("C:\\a", OSC_FAILED);
        f.pump_until("failed", |f| f.row(0).mark_state == 4);
        assert_eq!(f.row(0).tip.as_str(), "C:\\a\nFailed: rate limit");
    }

    #[test]
    fn eight_scales() {
        // Three marked tabs under Bar: question, failed and finished lines, standing still.
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let f = fx_with_prefs(
                Opts {
                    scale,
                    ..Opts::default()
                },
                |p| {
                    flare_settings::look::LookStore::new(p.clone())
                        .set_agent_indicator(flare_settings::look::AgentIndicator::Minimal);
                    flare_settings::agent_prefs::set_agent_rainbow(p, false);
                },
            );
            f.ctl.borrow_mut().set_animations_source(Rc::new(|| false));
            f.open_tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d"]);
            f.say("C:\\a", OSC_QUESTION);
            f.say("C:\\b", OSC_FAILED);
            f.say("C:\\c", OSC_DONE);
            f.pump_until("three marks", |f| (0..3).all(|i| f.row(i).mark_kind == 2));
            f.h.advance(50);
            let frame = f.h.render();
            let want = (3.0 * scale).round() as u32;
            for i in 0..3 {
                let row = f.row(i);
                let c = row.mark_color;
                let target = [c.red(), c.green(), c.blue(), 255];
                let top = (40.0 * scale) as u32;
                let bb = frame
                    .bbox_where(0, 0, frame.w, top, |p| Frame::near(p, target, 2))
                    .unwrap_or_else(|| panic!("no band for tab {i} at {scale}"));
                assert_eq!(
                    bb.3 - bb.1 + 1,
                    want,
                    "tab {i} at {scale}: the foot band is round(3 * scale) device pixels"
                );
            }
        }
    }

    // ---- restore with resume, the plugin, the close question, keys and the badge (Task 30) ----

    use crate::sessions::FixedSessions;
    use flare_win::taskbar::{Disc, OverlayShell};

    const SESSION_A: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
    const SESSION_B: &str = "0f0e0d0c-2222-4333-8444-955566667777";
    const TITLE_IDLE: &str = "\u{1b}]0;\u{2733} Claude Code\u{7}";
    const TITLE_CODEX: &str = "\u{1b}]0;[ ! ] Action Required | proj\u{7}";

    /// A pwsh that has a startup command to add the resume to (as the real one has).
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

    fn env_of(m: &flare_term::fake::FakeState, key: &str) -> Option<String> {
        m.spec()
            .env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    }

    fn lookup_of(l: &Arc<FixedSessions>) -> Option<Arc<dyn SessionLookup>> {
        let l: Arc<dyn SessionLookup> = l.clone();
        Some(l)
    }

    /// Records every overlay the taskbar was given: the disc size (None: cleared) and the words.
    #[derive(Clone, Default)]
    struct BadgeLog(Rc<RefCell<Vec<Overlay>>>);

    type Overlay = (Option<u32>, String);

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

    impl Fx {
        fn with_badge(&self) -> BadgeLog {
            let log = BadgeLog::default();
            self.ctl
                .borrow_mut()
                .set_taskbar_badge(TaskbarBadge::with_shell(Box::new(RecShell(log.clone()))));
            log
        }

        /// The tab ids in strip order.
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

        /// Steps the controller's timer body until `cond` holds (the reveal clock runs on it).
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

        fn agent_here(&self, id: &str) -> bool {
            self.ctl.borrow().hub.agent_here(id)
        }

        /// A Claude title in `cwd`'s shell, and the wait until the hub knows the agent.
        fn claude_in(&self, cwd: &str) -> String {
            let id = self.ids()[self
                .cwds()
                .iter()
                .position(|c| c == cwd)
                .expect("a tab in that folder")]
            .clone();
            self.say(cwd, TITLE_IDLE);
            self.pump_until("the agent to be here", |f| f.agent_here(&id));
            id
        }

        /// The process poll no longer finds the agent in `id`'s shell: it has exited. The answer is taken as
        /// production takes it, on the host's wake.
        fn agent_exits(&self, id: &str) {
            let mut c = self.ctl.borrow_mut();
            c.hub_mut().queue_poll_answer(id, false, None);
            c.host_wake();
        }
    }

    #[test]
    fn a_panel_claude_opened_closes_when_claude_leaves() {
        let (f, _h, _) = live(Opts::default());
        let a = f.claude_in("C:\\a");
        ctl_call(&f, show_text_op(&a, "report", Some("Report"))).unwrap();
        f.pump_until("the panel", |f| f.h.comp.get_panel_open());
        f.agent_exits(&a);
        f.pump_until("the panel to close", |f| !f.h.comp.get_panel_open());
        // What it showed is the user's now: the toggle chord brings it back, and it stays.
        assert!(f.ctl.borrow().panels.agent_tabs().is_empty());
        assert!(f.ctl.borrow_mut().panels.reopen(&a));
        f.ctl.borrow_mut().refresh();
        f.pump();
        assert!(f.h.comp.get_panel_open());
    }

    #[test]
    fn a_panel_the_user_opened_stays_when_claude_leaves() {
        let (f, _h, _) = live(Opts::default());
        let a = f.claude_in("C:\\a");
        ctl_call(&f, show_text_op(&a, "report", None)).unwrap();
        // The user shows a file of their own in the same panel: it is theirs from then on.
        let note = f.dir.path().join("mine.txt");
        std::fs::write(&note, "mine").unwrap(); // sync-ok: test code only
        assert!(f.ctl.borrow_mut().panels.show(
            &a,
            flare_surfaces::PanelKind::Text,
            &note.to_string_lossy()
        ));
        f.ctl.borrow_mut().refresh();
        f.pump_until("the panel", |f| f.h.comp.get_panel_open());
        f.agent_exits(&a);
        f.ctl.borrow_mut().refresh();
        f.pump();
        assert!(!f.agent_here(&a), "the agent has left");
        assert!(f.h.comp.get_panel_open(), "the user's panel stays");
    }

    #[test]
    fn the_restore_asks_the_lookup_for_claude_tabs_in_present_folders_only() {
        let lookup = FixedSessions::new(&[SESSION_A]);
        let f = fx_with(Opts {
            saved: vec![
                ("C:\\c", Some(AgentKind::Claude)),
                ("C:\\gone", Some(AgentKind::Claude)),
                ("C:\\p", None),
                ("C:\\x", Some(AgentKind::Codex)),
            ],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        f.start_restore_and_wait();
        // Wait for the three shells the test reads, not for a count: the gone folder's shell may be
        // one of the first three made, and the codex tab's would then not exist yet.
        f.pump_until("the spawns", |f| {
            ["C:\\c", "C:\\p", "C:\\x"]
                .iter()
                .all(|cwd| !f.made_with(cwd).is_empty())
        });
        assert_eq!(
            lookup.asked(),
            ["C:\\c"],
            "asked once, for the present Claude folder"
        );
        let claude = &f.made_with("C:\\c")[0];
        assert_eq!(
            env_of(claude, "FLARE_RESUME_ID").as_deref(),
            Some(SESSION_A)
        );
        let plain = &f.made_with("C:\\p")[0];
        assert!(env_of(plain, "FLARE_RESUME_ID").is_none());
        assert_eq!(plain.spec().args, resume_pwsh().args);
        let codex = &f.made_with("C:\\x")[0];
        assert!(env_of(codex, "FLARE_RESUME_ID").is_none());
        assert!(
            codex
                .spec()
                .args
                .last()
                .is_some_and(|a| a.ends_with("codex resume --last"))
        );
    }

    #[test]
    fn an_agy_tab_comes_back_as_agy_c_wearing_its_icon_from_the_first_frame() {
        let lookup = FixedSessions::new(&[SESSION_A]);
        let f = fx_with(Opts {
            saved: vec![("C:\\c", None), ("C:\\y", Some(AgentKind::Agy))],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        assert_eq!(f.row(1).agent, 3, "the agy arch from the first frame");
        assert!(f.row(1).loading);
        f.start_restore_and_wait();
        f.pump_until("the spawns", |f| !f.made_with("C:\\y").is_empty());
        assert!(lookup.asked().is_empty(), "agy asks Claude's store nothing");
        let agy = &f.made_with("C:\\y")[0];
        assert!(env_of(agy, "FLARE_RESUME_ID").is_none());
        assert_eq!(env_of(agy, "AGY_CLI_OSC7501").as_deref(), Some("on"));
        assert!(
            agy.spec()
                .args
                .last()
                .is_some_and(|a| a.ends_with("; agy -c"))
        );
        assert_eq!(
            f.ctl.borrow().model().agent_kind(&f.ids()[1]),
            Some(AgentKind::Agy)
        );
    }

    #[test]
    fn two_claude_tabs_in_one_folder_resume_the_newest_two() {
        let lookup = FixedSessions::new(&[SESSION_A, SESSION_B]);
        let f = fx_with(Opts {
            saved: vec![
                ("C:\\c", Some(AgentKind::Claude)),
                ("C:\\c", Some(AgentKind::Claude)),
            ],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        f.start_restore_and_wait();
        // Each tab's shell starts on its own worker thread, so the spawner's list is in finishing
        // order, not strip order. Read each tab's own shell (by its tab token), in strip order.
        let ids = f.ids();
        f.pump_until("each tab's shell", |f| {
            ids.iter().all(|id| f.shell_of_tab(id).is_some())
        });
        let got: Vec<Option<String>> = ids
            .iter()
            .map(|id| env_of(&f.shell_of_tab(id).expect("its shell"), "FLARE_RESUME_ID"))
            .collect();
        assert_eq!(
            got,
            [Some(SESSION_A.to_string()), Some(SESSION_B.to_string())]
        );
    }

    #[test]
    fn no_session_means_a_plain_shell() {
        let lookup = FixedSessions::new(&[]);
        let f = fx_with(Opts {
            saved: vec![("C:\\c", Some(AgentKind::Claude))],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        f.start_restore_and_wait();
        f.wait_made("C:\\c");
        assert!(env_of(&f.made_with("C:\\c")[0], "FLARE_RESUME_ID").is_none());
        assert!(!f.h.comp.get_skeleton_shown(), "no skeleton");
        let id = f.ids()[0].clone();
        assert!(f.ctl.borrow().model().agent_kind(&id).is_none());
    }

    #[test]
    fn a_bad_resume_id_never_reaches_a_command_line_from_the_lookup() {
        let lookup = FixedSessions::new(&["x; rm -rf"]);
        let f = fx_with(Opts {
            saved: vec![("C:\\c", Some(AgentKind::Claude))],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        f.start_restore_and_wait();
        f.wait_made("C:\\c");
        let m = &f.made_with("C:\\c")[0];
        assert!(env_of(m, "FLARE_RESUME_ID").is_none());
        assert_eq!(m.spec().args, resume_pwsh().args, "plain args");
        assert!(!f.h.comp.get_skeleton_shown());
    }

    #[test]
    fn the_front_agent_tab_wears_the_skeleton_until_the_agent_paints() {
        let lookup = FixedSessions::new(&[SESSION_A]);
        let f = fx_with(Opts {
            saved: vec![("C:\\c", Some(AgentKind::Claude))],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        assert!(f.h.comp.get_skeleton_shown(), "from the first frame");
        f.start_restore_and_wait();
        f.wait_made("C:\\c");
        assert!(f.h.comp.get_skeleton_shown());
        assert!(!f.h.comp.get_skeleton_leaving());
        // The shell's own text comes first, then the agent paints.
        f.say("C:\\c", "PS C:\\c> claude --resume\r\n");
        f.say("C:\\c", "\u{1b}]0;claude\u{7}");
        f.say("C:\\c", TITLE_IDLE);
        f.tick_until("the fade to start", |f| f.h.comp.get_skeleton_leaving());
        assert!(f.h.comp.get_skeleton_shown(), "still there while it fades");
        let id = f.ids()[0].clone();
        let handle = f.ctl.borrow().deps.host.handle(&id).expect("a handle");
        let grid = handle.snapshot();
        let text: String = (0..grid.rows())
            .flat_map(|r| (0..grid.cols()).map(move |c| (c, r)))
            .map(|(c, r)| grid.cell(c, r).ch)
            .collect();
        assert!(
            !text.contains("PS C:"),
            "the clear left no shell text: {text:?}"
        );
        let at = f.clock.get();
        f.clock.set(at + 181);
        f.tick_until("the skeleton to go", |f| !f.h.comp.get_skeleton_shown());
        assert!(!f.h.comp.get_skeleton_leaving());
    }

    #[test]
    fn a_tab_behind_shows_its_agent_icon_and_ring_at_once() {
        let lookup = FixedSessions::new(&[SESSION_A]);
        let f = fx_with(Opts {
            saved: vec![
                ("C:\\c", Some(AgentKind::Claude)),
                ("C:\\x", Some(AgentKind::Codex)),
            ],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        let behind = f.row(1);
        assert_eq!(behind.agent, 2, "the saved agent from the first frame");
        assert!(behind.loading);
        f.start_restore_and_wait();
        f.pump_until("the spawns", |f| f.sp.made().len() >= 2);
        let behind = f.row(1);
        assert_eq!(behind.agent, 2);
        assert!(behind.loading, "the ring until the agent paints");
        assert!(
            f.h.comp.get_skeleton_shown(),
            "the skeleton is for the front tab"
        );
    }

    #[test]
    fn restored_tabs_carry_no_done_question_or_failed() {
        let lookup = FixedSessions::new(&[SESSION_A, SESSION_B]);
        let f = fx_with(Opts {
            saved: vec![
                ("C:\\c", Some(AgentKind::Claude)),
                ("C:\\d", Some(AgentKind::Claude)),
            ],
            lookup: lookup_of(&lookup),
            shells: Some(vec![resume_pwsh()]),
            ..Opts::default()
        });
        f.start_restore_and_wait();
        // Wait for each tab's own shell: a count of spawns says nothing about whose they are, and `say`
        // reads the first shell of a cwd.
        f.wait_made("C:\\c");
        f.wait_made("C:\\d");
        f.ctl.borrow_mut().hub_mut().set_looking(None, false);
        f.say("C:\\c", TITLE_IDLE);
        f.say("C:\\d", TITLE_IDLE);
        f.clock.set(f.clock.get() + 3_000);
        std::thread::sleep(Duration::from_millis(100));
        f.pump();
        f.ctl.borrow_mut().hub_mut().tick();
        let c = f.ctl.borrow();
        let i = c.hub.indicator();
        assert!(i.done_ids().is_empty(), "no Finished from a resume");
        assert!(i.question_ids().is_empty());
        assert!(i.failed_ids().is_empty());
        assert!(i.working_ids().is_empty());
    }

    #[test]
    fn the_saved_strip_records_an_agent_only_for_claude_and_codex() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        f.claude_in("C:\\a");
        f.pump_until("the saved agent", |f| {
            f.ctl.borrow().model().agent_kind(&ids[0]) == Some(AgentKind::Claude)
        });
        let saved = f.ctl.borrow().model().saved();
        assert_eq!(saved.tabs[0].agent, Some(AgentKind::Claude));
        assert_eq!(saved.tabs[1].agent, None);
    }

    #[test]
    fn a_tab_hosting_an_agent_asks_naming_it_and_working_for() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a"]);
        f.claude_in("C:\\a");
        f.ctl.borrow_mut().request_close(&ids[0]);
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Close the tab and end the agent?"
        );
        // PT's confirm is the solid accent, never the danger fill; a click opened it, so no ring.
        assert!(!f.h.comp.get_dialog_danger(), "no danger fill");
        assert!(!f.h.comp.get_dialog_by_key(), "opened by a click");
        let body = {
            let c = f.ctl.borrow();
            ask_view(c.ask.as_ref().expect("an ask"), "#ffffff").body
        };
        assert!(body.contains("Claude"), "{body}");
        f.h.comp.invoke_dialog_cancel();

        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.ctl.borrow().hub.working(&ids[0]));
        f.clock.set(f.clock.get() + 125_000);
        f.ctl.borrow_mut().request_close(&ids[0]);
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Stop the agent and close?"
        );
        let body = {
            let c = f.ctl.borrow();
            ask_view(c.ask.as_ref().expect("an ask"), "#ffffff").body
        };
        assert!(body.contains("2 minutes"), "{body}");
        f.h.comp.invoke_dialog_cancel();
        assert_eq!(f.rows(), 1, "Cancel keeps the tab");
    }

    #[test]
    fn a_codex_tab_asks_naming_codex() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a"]);
        // Codex has no hooks: it is found by the poll, whose answer this stands in for.
        f.ctl
            .borrow_mut()
            .model_mut()
            .set_agent(&ids[0], Some(AgentKind::Codex));
        f.ctl.borrow_mut().request_close(&ids[0]);
        let body = {
            let c = f.ctl.borrow();
            ask_view(c.ask.as_ref().expect("an ask"), "#ffffff").body
        };
        assert!(body.contains("Codex"), "{body}");
    }

    #[test]
    fn the_tabs_saved_at_quit_keep_their_agent_when_the_killed_shells_report_it_gone() {
        let f = fx();
        f.start_restore_and_wait();
        f.open_tabs(&["C:\\a", "C:\\b"]);
        let a = f.claude_in("C:\\a");
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::Close
        );
        let agents = |f: &Fx| {
            let raw = std::fs::read_to_string(f.dir.path().join("tabs.json")).unwrap(); // sync-ok: test code only, never the app
            flare_tabs::restore::parse_tabs(&raw)
                .tabs
                .iter()
                .map(|t| t.agent)
                .collect::<Vec<_>>()
        };
        assert_eq!(agents(&f), vec![Some(AgentKind::Claude), None]);
        // The shells are killed: the poll says the agent left, and the event loop still refreshes.
        f.agent_exits(&a);
        f.ctl.borrow_mut().refresh();
        std::thread::sleep(Duration::from_millis(120)); // sync-ok: test code, past the store's 30 ms delay
        f.pump();
        assert_eq!(agents(&f), vec![Some(AgentKind::Claude), None]);
    }

    #[test]
    fn the_window_close_is_held_only_while_an_agent_works_by_the_hub() {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        f.claude_in("C:\\a");
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::Close,
            "idle"
        );
        let f = fx();
        let ids2 = f.open_tabs(&["C:\\a", "C:\\b"]);
        f.say("C:\\a", OSC_WORKING);
        f.say("C:\\b", OSC_WORKING);
        f.pump_until("both working", |f| {
            let c = f.ctl.borrow();
            c.hub.working(&ids2[0]) && c.hub.working(&ids2[1])
        });
        f.clock.set(f.clock.get() + 5_000);
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::KeepOpen
        );
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Stop the agent and close the window?"
        );
        let c = f.ctl.borrow();
        assert_eq!(
            c.ask.as_ref().expect("an ask").others,
            1,
            "the other is counted"
        );
        drop(c);
        let _ = ids;
    }

    #[test]
    fn keep_agent_sessions_open() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        let a = f.ids()[0].clone();
        let d = || ctrl("d", "KeyD");
        let c = || ctrl("c", "KeyC");
        // A plain shell: both pass.
        assert_eq!(f.key(d()), KeyOutcome::Pty);
        assert_eq!(f.key(c()), KeyOutcome::Pty);
        f.claude_in("C:\\a");
        assert_eq!(
            f.key(d()),
            KeyOutcome::Consumed,
            "Ctrl+D never reaches an idle agent"
        );
        assert_eq!(
            f.key(c()),
            KeyOutcome::Consumed,
            "Ctrl+C is taken while idle"
        );
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("working", |f| f.ctl.borrow().hub.working(&a));
        assert_eq!(f.key(c()), KeyOutcome::Pty, "Ctrl+C passes while working");
        flare_settings::agent_prefs::set_agent_guard(&f.prefs, false);
        assert_eq!(f.key(d()), KeyOutcome::Pty, "the guard off: both pass");
    }

    #[test]
    fn ctrl_c_copies_a_grid_selection_over_an_idle_agent() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        let a = f.ctl.borrow().front_id().unwrap();
        f.claude_in("C:\\a");
        let c = || ctrl("c", "KeyC");
        assert_eq!(
            f.key(c()),
            KeyOutcome::Consumed,
            "no selection: taken while idle"
        );
        // Flare's own selection in the grid (the agent does not report the mouse).
        let h = f.ctl.borrow().deps.host.handle(&a).unwrap();
        h.with_emu(|e| {
            e.feed(b"some output\r\n");
            e.sel_apply(flare_term::sel_machine::SelOut::Set(
                flare_term::sel_machine::SelSpec {
                    block: false,
                    start: flare_term::sel_machine::Anchor { row: 0, b: 0 },
                    end: flare_term::sel_machine::Anchor { row: 0, b: 4 },
                },
            ));
        });
        assert!(h.with_emu_quiet(|e| e.has_selection()));
        assert_eq!(
            f.key(c()),
            KeyOutcome::Consumed,
            "Ctrl+C with a selection is the copy, not the guard"
        );
        f.pump_until("the copy", |f| {
            f.clip.read_text().is_some_and(|t| !t.is_empty())
        });
    }

    #[test]
    fn agent_hotkeys() {
        let f = fx();
        f.open_tabs(&["C:\\a", "C:\\b"]);
        assert_eq!(f.ctl.borrow().model().active().unwrap().cwd, "C:\\b");
        assert_eq!(f.key(ctrl("PageUp", "PageUp")), KeyOutcome::Consumed);
        assert_eq!(f.ctl.borrow().model().active().unwrap().cwd, "C:\\a");
        set_agent_keys(&f.prefs, false);
        assert_eq!(f.key(ctrl("PageDown", "PageDown")), KeyOutcome::Pty);
        assert_eq!(
            f.ctl.borrow().model().active().unwrap().cwd,
            "C:\\a",
            "with the hotkeys off it reaches the shell"
        );
    }

    #[test]
    fn shift_enter_is_a_newline_only_where_an_agent_is_armed() {
        let f = fx();
        f.open_tabs(&["C:\\a"]);
        let m = f.made_with("C:\\a")[0].clone();
        let shift_enter = || KeyEvent::new("Enter", "Enter").shift();
        let wait_for = |want: &[u8]| {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(5) {
                if m.written().ends_with(want) {
                    return;
                }
                f.pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("{:?} was not written: {:?}", want, m.written());
        };
        // No agent here: the key goes to the shell as an ordinary Enter (the window sends it).
        assert_eq!(f.key(shift_enter()), KeyOutcome::Pty);
        assert!(m.written().is_empty());
        // An agent only the indicator knows (no arm yet): still Enter.
        f.say("C:\\a", OSC_WORKING);
        f.pump_until("the hook", |f| f.agent_here(&f.ids()[0]));
        assert_eq!(f.key(shift_enter()), KeyOutcome::Pty);
        f.say("C:\\a", TITLE_IDLE);
        f.pump_until("claude armed", |f| {
            f.ctl.borrow().agent_view.armed(&f.ids()[0]) == Some(AgentKind::Claude)
        });
        assert_eq!(f.key(shift_enter()), KeyOutcome::Consumed);
        wait_for(b"\n");
        f.say("C:\\a", TITLE_CODEX);
        f.pump_until("codex armed", |f| {
            f.ctl.borrow().agent_view.armed(&f.ids()[0]) == Some(AgentKind::Codex)
        });
        assert_eq!(f.key(shift_enter()), KeyOutcome::Consumed);
        wait_for(b"\\\r");
    }

    #[test]
    fn the_badge_counts_marked_tabs_and_clears() {
        let f = fx_with(Opts {
            scale: 2.0,
            ..Opts::default()
        });
        let log = f.with_badge();
        let ids = f.open_tabs(&["C:\\a", "C:\\b", "C:\\c"]);
        f.say("C:\\a", OSC_DONE);
        f.say("C:\\b", OSC_QUESTION);
        f.pump_until("two marks", |f| {
            let c = f.ctl.borrow();
            c.hub.indicator().done_ids().contains(&ids[0])
                && c.hub.indicator().question_ids().contains(&ids[1])
        });
        let last = log.0.borrow().last().cloned().expect("an overlay");
        assert_eq!(last.1, "2 tabs need a look");
        assert_eq!(last.0, Some(32), "the disc is 16 x the scale");
        // Looking at one of them.
        f.ctl
            .borrow_mut()
            .hub_mut()
            .set_looking(Some(&ids[0]), true);
        f.ctl.borrow_mut().refresh();
        let last = log.0.borrow().last().cloned().expect("an overlay");
        assert_eq!(last.1, "1 tab needs a look");
        // The setting off.
        flare_settings::agent_prefs::set_taskbar_badge_on(&f.prefs, false);
        f.ctl.borrow_mut().refresh();
        assert_eq!(log.0.borrow().last().cloned(), Some((None, String::new())));
        // On again, and nothing marked: cleared.
        flare_settings::agent_prefs::set_taskbar_badge_on(&f.prefs, true);
        f.ctl.borrow_mut().refresh();
        assert_eq!(
            log.0.borrow().last().map(|l| l.1.clone()).as_deref(),
            Some("1 tab needs a look")
        );
        f.say("C:\\b", OSC_WORKING);
        f.pump_until("the answer", |f| {
            f.ctl.borrow().hub.indicator().question_ids().is_empty()
        });
        f.ctl
            .borrow_mut()
            .hub_mut()
            .set_looking(Some(&ids[2]), true);
        f.ctl.borrow_mut().refresh();
        assert_eq!(
            log.0.borrow().last().cloned().map(|l| l.0),
            Some(None),
            "nothing marked: the overlay is cleared"
        );
    }

    /// Coming back to the window draws the badge anew even when the count did not change: Explorer may have
    /// restarted (which takes every overlay away), and no subclass listens for its `TaskbarButtonCreated`.
    #[test]
    fn a_gained_focus_draws_the_same_badge_again() {
        let f = fx();
        let log = f.with_badge();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        f.say("C:\\a", OSC_DONE);
        f.pump_until("a mark", |f| {
            f.ctl.borrow().hub.indicator().done_ids().contains(&ids[0])
        });
        f.ctl.borrow_mut().on_focus(false);
        let before = log.0.borrow().len();
        f.ctl.borrow_mut().refresh();
        assert_eq!(log.0.borrow().len(), before, "the same count draws nothing");
        f.ctl.borrow_mut().on_focus(true);
        let drawn: Vec<Overlay> = log.0.borrow()[before..].to_vec();
        assert!(
            drawn.iter().any(|o| o.1 == "1 tab needs a look"),
            "drawn again on focus: {drawn:?}"
        );
    }

    /// A tab "C:\a" finished on the front with the window away.
    fn finished_away() -> (Fx, Vec<String>) {
        let f = fx();
        let ids = f.open_tabs(&["C:\\a", "C:\\b"]);
        f.ctl.borrow_mut().model_mut().pick(&ids[0]);
        f.ctl.borrow_mut().refresh();
        f.ctl.borrow_mut().on_focus_lost();
        f.say("C:\\a", OSC_WORKING);
        f.say("C:\\a", OSC_DONE);
        f.pump_until("the finished mark", |f| {
            f.ctl.borrow().hub.indicator().done_ids().contains(&ids[0])
        });
        (f, ids)
    }

    #[test]
    fn coming_back_and_pointer_moves_keep_a_held_mark() {
        let (f, ids) = finished_away();
        f.clock.set(f.clock.get() + 2_000);
        f.ctl.borrow_mut().on_cursor(10.0, 10.0);
        f.ctl.borrow_mut().on_cursor(20.0, 20.0);
        assert!(f.ctl.borrow().hub.indicator().done_ids().contains(&ids[0]));
        f.ctl.borrow_mut().on_focus(true);
        assert!(
            f.ctl.borrow().hub.indicator().done_ids().contains(&ids[0]),
            "focus returning keeps it"
        );
    }

    fn held(f: &Fx, id: &str) -> bool {
        f.ctl
            .borrow()
            .hub
            .indicator()
            .done_ids()
            .contains(&id.to_string())
    }

    #[test]
    fn a_pick_of_another_tab_takes_a_held_mark_down_on_the_tab_left() {
        let (f, ids) = finished_away();
        f.ctl.borrow_mut().on_focus(true);
        assert!(held(&f, &ids[0]), "coming back keeps it");
        f.ctl.borrow_mut().overflow_picked(&ids[1]);
        assert!(!held(&f, &ids[0]), "no background mark lingers");
    }

    #[test]
    fn a_pick_of_the_same_tab_keeps_a_held_mark() {
        let (f, ids) = finished_away();
        f.ctl.borrow_mut().on_focus(true);
        f.ctl.borrow_mut().overflow_picked(&ids[0]);
        f.ctl.borrow_mut().refresh();
        assert!(held(&f, &ids[0]));
    }

    #[test]
    fn a_switch_with_no_click_takes_a_held_mark_down() {
        // A key or a wheel moves the front tab with no pick call: the refresh reads it.
        let (f, ids) = finished_away();
        f.ctl.borrow_mut().on_focus(true);
        f.ctl.borrow_mut().model_mut().pick(&ids[1]);
        f.ctl.borrow_mut().refresh();
        assert!(!held(&f, &ids[0]));
    }

    #[test]
    fn closing_another_tab_keeps_a_held_mark() {
        let (f, ids) = finished_away();
        f.ctl.borrow_mut().on_focus(true);
        f.ctl.borrow_mut().model_mut().close(&ids[1]);
        f.ctl.borrow_mut().refresh();
        assert!(held(&f, &ids[0]));
    }

    // ---- command help (Task 43) ----

    fn f1() -> KeyEvent {
        KeyEvent::new("F1", "F1")
    }

    fn help_open(f: &Fx) -> bool {
        f.h.comp.global::<HelpBridge>().get_open()
    }

    fn help_chips_on(f: &Fx) -> Vec<String> {
        f.h.comp
            .global::<HelpBridge>()
            .get_chips()
            .iter()
            .filter(|c| c.on)
            .map(|c| c.id.to_string())
            .collect()
    }

    #[test]
    fn f1_opens_even_over_a_focused_shell_and_over_settings() {
        let (f, _h, fake) = live(Opts::default());
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(help_open(&f));
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(!help_open(&f), "F1 again closes it");
        // Over the Settings page the chord table matches F1 just the same.
        f.ctl.borrow_mut().toggle_settings();
        assert_eq!(f.h.comp.get_front_kind(), 1);
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(help_open(&f));
        assert!(fake.written().is_empty(), "nothing reached the shell");
    }

    #[test]
    fn a_key_over_the_settings_page_goes_to_slint_not_the_void() {
        // The e2e found it (colourPicker, settingsSearch): with the Settings page in front there is no
        // shell, so a key that is no chord was consumed and never reached the find field, a code field
        // or the colour popover's Escape.
        let (f, _h, fake) = live(Opts::default());
        assert_eq!(
            f.key(KeyEvent::new("a", "KeyA")),
            KeyOutcome::Pty,
            "a shell: the shell's"
        );
        f.ctl.borrow_mut().toggle_settings();
        assert_eq!(f.h.comp.get_front_kind(), 1);
        assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Slint);
        assert_eq!(f.key(KeyEvent::new("Escape", "Escape")), KeyOutcome::Slint);
        assert_eq!(f.key(KeyEvent::new("Enter", "Enter")), KeyOutcome::Slint);
        // A chord is still the app's, and nothing reached the shell.
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(fake.written().is_empty(), "nothing reached the shell");
    }

    #[test]
    fn help_off_means_off() {
        let (f, _h, _fake) = live(Opts::default());
        assert!(f.h.comp.get_help_visible());
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(help_open(&f));
        flare_settings::help_prefs::set_help_enabled(&f.prefs, false);
        f.pump();
        assert!(!help_open(&f), "switched off: closed at once");
        assert!(!f.h.comp.get_help_visible(), "the button is gone");
        assert!(!f.ctl.borrow().key_context().help);
        // F1 is no chord now: the shell hears it.
        assert_eq!(f.key(f1()), KeyOutcome::Pty);
        assert!(!help_open(&f));
        // The terminal menu has no Command help row.
        f.ptr(0, 2, cell_css(2, 1, false));
        f.ptr(1, 2, cell_css(2, 1, false));
        assert!(f.h.comp.get_menu_open());
        let rows = f.h.comp.get_menu_rows();
        assert!(rows.iter().all(|r| r.label != "Command help"));
    }

    #[test]
    fn the_menu_row_opens_it() {
        let (f, _h, _) = live(Opts::default());
        f.ptr(0, 2, cell_css(2, 1, false));
        f.ptr(1, 2, cell_css(2, 1, false));
        assert!(f.h.comp.get_menu_open());
        let rows = f.h.comp.get_menu_rows();
        let last = rows.row_count() - 1;
        assert_eq!(rows.row_data(last).unwrap().label, "Command help");
        f.h.comp.invoke_menu_picked(last as i32);
        assert!(!f.h.comp.get_menu_open());
        assert!(help_open(&f));
    }

    #[test]
    fn opening_starts_on_the_front_tabs_shell() {
        let cmd = ShellDef {
            id: "cmd".to_string(),
            name: "Command Prompt".to_string(),
            exe: "cmd.exe".to_string(),
            args: Vec::new(),
        };
        let (f, _h, _) = live(Opts {
            shells: Some(vec![cmd]),
            ..Opts::default()
        });
        f.key(f1());
        assert_eq!(help_chips_on(&f), ["cmd"]);
        // With Settings in front there is no shell: the chip picked by hand, else PowerShell.
        f.key(f1());
        f.ctl.borrow_mut().toggle_settings();
        f.key(f1());
        assert_eq!(help_chips_on(&f), ["powershell"]);
    }

    #[test]
    fn the_later_opened_of_help_and_find_is_on_top() {
        let (f, _h, _) = live(Opts::default());
        // find first, help after: help is on top
        f.key(ctrl("f", "KeyF"));
        assert!(f.h.comp.get_find_open());
        f.key(f1());
        assert!(f.h.comp.get_help_on_top());
        // help stays open; the bar closes and opens again after it: the bar is on top
        f.key(ctrl("f", "KeyF"));
        assert!(!f.h.comp.get_find_open());
        f.key(ctrl("f", "KeyF"));
        assert!(f.h.comp.get_find_open());
        assert!(!f.h.comp.get_help_on_top());
    }

    #[test]
    fn the_panel_keys_stay_in_the_panel_while_it_has_the_keyboard() {
        let (f, _h, fake) = live(Opts::default());
        f.h.comp
            .window()
            .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
        f.key(f1());
        f.h.advance(1);
        f.h.advance(80);
        assert!(
            f.h.comp.get_find_input_focused(),
            "the search field has the keys"
        );
        assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Slint);
        // chords still act
        let rows = f.rows();
        assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
        assert_eq!(f.rows(), rows + 1);
        assert!(fake.written().is_empty());
    }

    // ---- the update install question and the preview hand-off (M9 Task 30) ----

    type UpJobs = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;

    struct NoWait;
    impl flare_update::preview::Waiter for NoWait {
        fn wait(&self, _ms: u64) {}
    }

    fn attach_update(
        f: &Fx,
        mode: flare_update::engine::Mode,
    ) -> (Arc<flare_update::engine::Engine>, UpJobs) {
        use flare_update::engine::{CondvarSleeper, Engine};
        use flare_update::feed::{FakeFeed, Feed};
        let q = f.q.clone();
        let post: Post = Arc::new(move |m| q.lock().unwrap().push_back(m));
        let feed: Arc<dyn Feed> = Arc::new(FakeFeed::new().offering("99.0.0", "* a thing"));
        let engine = Engine::new(
            mode,
            feed,
            "0.1.0",
            None,
            Arc::new(crate::update_ctl::PostEvents { post }),
            Arc::new(CondvarSleeper::new()),
            Arc::new(NoWait),
        );
        let jobs: UpJobs = Rc::default();
        let j = jobs.clone();
        AppCtl::attach_update(
            &f.h.comp,
            &f.ctl,
            UpdateParts {
                engine: engine.clone(),
                current_version: "0.1.0".to_string(),
                e2e_dir: None,
                spawn: Rc::new(move |job| j.borrow_mut().push(job)),
            },
        );
        (engine, jobs)
    }

    fn with_a_working_agent(f: &Fx) {
        let id = f.ctl.borrow_mut().open_folder("C:\\a");
        {
            let mut c = f.ctl.borrow_mut();
            c.model_mut().set_agent(&id, Some(AgentKind::Claude));
            c.set_working(&id, Some(0));
        }
        f.clock.set(5_000);
    }

    #[test]
    fn the_close_question_puts_the_help_panel_away_and_f1_waits() {
        let f = fx();
        with_a_working_agent(&f);
        assert_eq!(f.key(f1()), KeyOutcome::Consumed);
        assert!(help_open(&f));
        let id = f.ctl.borrow().front_id().expect("a tab in front");
        f.ctl.borrow_mut().request_close(&id);
        assert!(f.h.comp.get_dialog_open(), "the close question is up");
        assert!(
            !help_open(&f),
            "the panel is put away, so the question is never underneath it"
        );
        f.key(f1());
        assert!(!help_open(&f), "F1 does not open it over a question");
    }

    #[test]
    fn the_update_install_asks_first_when_an_agent_works() {
        use flare_update::engine::Mode;
        let f = fx();
        with_a_working_agent(&f);
        let (engine, jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        f.h.comp.invoke_update_chip_clicked();
        assert!(f.h.comp.get_update_open());
        f.h.comp.invoke_update_install();
        assert!(f.h.comp.get_dialog_open(), "the close question is up");
        assert_eq!(
            f.h.comp.get_dialog_title().as_str(),
            "Stop the agent and install the update?"
        );
        assert_eq!(
            f.h.comp.get_dialog_primary().as_str(),
            "Install and restart"
        );
        assert!(f.ctl.borrow().ask.as_ref().expect("an ask").install);
        assert!(!f.h.comp.get_update_open(), "the update window steps aside");
        assert_eq!(engine.calls().installs, 0);
        assert!(jobs.borrow().is_empty());
        // No: nothing starts and the window comes back.
        f.h.comp.invoke_dialog_cancel();
        assert!(!f.h.comp.get_dialog_open());
        assert!(f.h.comp.get_update_open());
        assert!(jobs.borrow().is_empty());
        // Yes: the install starts, and the app does not quit yet.
        f.h.comp.invoke_update_install();
        f.h.comp.invoke_dialog_confirm();
        assert_eq!(jobs.borrow().len(), 1, "a worker was started");
        assert!(f.h.comp.get_update_running());
        assert!(f.h.comp.get_update_open());
        assert_eq!(f.log.borrow().quit, 0);
        assert!(!f.ctl.borrow().ended);
    }

    #[test]
    fn a_tab_close_question_puts_the_update_window_away() {
        use flare_update::engine::Mode;
        let f = fx();
        with_a_working_agent(&f);
        let (engine, _jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        f.h.comp.invoke_update_chip_clicked();
        assert!(f.h.comp.get_update_open(), "the update window is up");
        let id = f.ctl.borrow().front_id().expect("a tab in front");
        f.ctl.borrow_mut().request_close(&id);
        assert!(f.h.comp.get_dialog_open(), "the close question is up");
        assert!(
            !f.h.comp.get_update_open(),
            "the update window gives way to the question"
        );
        // Cancelled: the window comes back, nothing installs, and the tab stays.
        f.h.comp.invoke_dialog_cancel();
        assert!(!f.h.comp.get_dialog_open());
        assert!(f.h.comp.get_update_open(), "the window comes back");
        assert_eq!(engine.calls().installs, 0);
        assert_eq!(f.ctl.borrow().model().state.tabs.len(), 1);
    }

    #[test]
    fn the_update_install_with_no_agent_starts_at_once_and_quits_after_the_apply() {
        use flare_update::engine::Mode;
        let f = fx();
        f.ctl.borrow_mut().open_folder("C:\\a");
        let (engine, jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        f.h.comp.invoke_update_chip_clicked();
        f.h.comp.invoke_update_install();
        assert!(!f.h.comp.get_dialog_open(), "nothing to ask");
        assert!(f.h.comp.get_update_running());
        let job = jobs.borrow_mut().pop().expect("the install worker");
        job();
        f.pump();
        assert_eq!(f.log.borrow().quit, 0, "not before the 400 ms");
        f.h.advance(400);
        assert_eq!(f.log.borrow().quit, 1);
        assert!(f.ctl.borrow().ended, "the end of life ran first");
    }

    #[test]
    fn an_agreed_install_lets_the_close_through_even_with_an_agent_working() {
        use flare_update::engine::Mode;
        let f = fx();
        with_a_working_agent(&f);
        let (engine, jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        f.h.comp.invoke_update_install();
        f.h.comp.invoke_dialog_confirm();
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::KeepOpen,
            "not agreed before the installer has started"
        );
        f.h.comp.invoke_dialog_cancel();
        let job = jobs.borrow_mut().pop().expect("the install worker");
        job();
        assert_eq!(
            f.ctl.borrow_mut().on_close_requested(),
            CloseDecision::Close,
            "agreed at the apply"
        );
    }

    #[test]
    fn a_second_launch_with_the_flag_previews_in_the_running_app() {
        use flare_update::engine::Mode;
        let f = fx();
        f.start_restore_and_wait();
        let (engine, _jobs) = attach_update(&f, Mode::Off);
        let tabs = f.rows();
        f.ctl.borrow_mut().handoff(Handoff {
            cwd: f.dir.path().to_string_lossy().into_owned(),
            args: vec!["--preview-update".to_string()],
        });
        f.pump();
        assert_eq!(f.log.borrow().raised, 1);
        assert!(f.h.comp.get_update_chip_shown(), "the preview offer is up");
        assert!(
            f.h.comp
                .get_update_chip_tip()
                .starts_with("Preview: see what")
        );
        assert_eq!(f.rows(), tabs, "no tab is opened");
        assert_eq!(engine.calls().installs, 0);
        // With the program's name in front, as a second launch really passes it.
        let g = fx();
        g.start_restore_and_wait();
        let (_e, _j) = attach_update(&g, Mode::Off);
        let tabs = g.rows();
        g.ctl.borrow_mut().handoff(Handoff {
            cwd: g.dir.path().to_string_lossy().into_owned(),
            args: vec!["flare.exe".to_string(), "--preview-update".to_string()],
        });
        g.pump();
        assert!(g.h.comp.get_update_chip_shown());
        assert_eq!(g.rows(), tabs);
    }

    #[test]
    fn a_preview_request_changes_nothing_while_a_real_offer_is_present() {
        use flare_update::engine::Mode;
        let f = fx();
        f.start_restore_and_wait();
        let (engine, _jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        assert_eq!(f.h.comp.get_update_chip_label().as_str(), "Update 99.0.0");
        f.ctl.borrow_mut().handoff(Handoff {
            cwd: f.dir.path().to_string_lossy().into_owned(),
            args: vec!["--preview-update".to_string()],
        });
        f.pump();
        assert_eq!(f.h.comp.get_update_chip_label().as_str(), "Update 99.0.0");
        assert!(
            f.h.comp
                .get_update_chip_tip()
                .starts_with("See what is new")
        );
    }

    #[test]
    fn the_chords_are_swallowed_while_the_update_window_is_up() {
        use flare_update::engine::Mode;
        let f = fx();
        f.start_restore_and_wait();
        let (engine, _jobs) = attach_update(&f, Mode::Live);
        engine.tick();
        f.pump();
        f.h.comp.invoke_update_chip_clicked();
        assert!(f.h.comp.get_update_open());
        assert!(f.h.comp.get_chrome_has_focus(), "its keys are the window's");
        let rows = f.rows();
        f.key(ctrl("t", "KeyT"));
        assert_eq!(f.rows(), rows, "no new tab behind the window");
        f.h.comp.invoke_update_cancel();
        f.key(ctrl("t", "KeyT"));
        assert_eq!(f.rows(), rows + 1, "and back to normal after");
    }
}
