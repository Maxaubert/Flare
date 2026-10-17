//! The one controller of the terminal's selection, copy, paste, find and links (Task 23).
//!
//! TermUi holds no Slint types. It is driven by plain structs (a pointer event, a key, a tick) and
//! calls the closures in `TermDeps`, so every behaviour is tested without a window or a shell.
//! Positions arrive in CSS px relative to the grid area; every hit test is in whole device pixels.
//! Source: PT `TerminalPanel.tsx` (`attachClickCaret`, `deleteSelection`, `termContextAt`, the key
//! handler, `pasteHere`), `App.tsx` (the menu, the drop, the find chord), xterm `SelectionService`.
#![allow(dead_code)] // wired into the window by Task 24; remove this line then

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use flare_agent::keys::{AgentView, newline_key};
use flare_render::decor::{Decor, DecorInk};
use flare_render::hit::{Hit, hit_cell, past_edge_px};
use flare_render::metrics::CellMetrics;
use flare_shells::paste::{ClipboardState, path_shell_of};
use flare_term::app_copy::AppCopyWatch;
use flare_term::click_caret::MouseTracking;
use flare_term::emulator::{DecorReq, TermEvent};
use flare_term::find;
use flare_term::handle::TermHandle;
use flare_term::keys::{KeyEvent, TermKeyAction, is_key};
use flare_term::link_scan::{LinkMark, LinkTarget};
use flare_term::link_target::{LinkAction, TermPathAt, click_action};
use flare_term::mouse_report::{MouseButton, MouseEvt, MouseKind, MouseMods, encode_mouse};
use flare_term::osc52::OSC52_MAX;
use flare_term::path_links::{Ask, BATCH_MS, Found, PathLinks};
use flare_term::path_open::{
    OpenMode, PathHit, PathOpeners, StatFn, StatGate, StatGateOptions, open_term_path, path_kinds,
};
use flare_term::sel_machine::{
    ClickCounter, DRAG_SCROLL_MS, Drag, Press, SelMachine, SelOut, ViewBox,
};
use flare_term::web_url::is_web_url;
use flare_win::clipboard_port::{COPY_MAX, ClipboardPort};
use flare_win::dblclick::DoubleClick;

use crate::badge::{BadgeClock, BadgePhase};
use crate::clip_flow::{
    AppCopyAdapter, PasteAction, PasteMode, PasteReq, copy_async, paste_action, read_async,
};
use crate::drop_flow::{DROP_QUIET_MS, DropBatcher, type_dropped};
use crate::term_menu::{TermMenuAct, TermMenuAt, TermMenuRow, term_menu_rows};

/// Which mouse cursor the grid shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Text,
    Pointer,
    Default,
}

/// What a worker tells the UI thread.
pub enum TermMsg {
    Clip(PasteReq, ClipboardState),
    Copied(bool),
    PathAnswer(Ask, Option<Vec<Option<PathHit>>>),
}

/// Everything TermUi needs from the outside, injected.
pub struct TermDeps {
    pub clipboard: Arc<dyn ClipboardPort>,
    pub openers: Arc<dyn PathOpeners + Send + Sync>,
    /// Called only after `is_web_url`.
    pub open_url: Rc<dyn Fn(&str)>,
    /// Worker to UI thread.
    pub post: Arc<dyn Fn(TermMsg) + Send + Sync>,
    pub clock_ms: Rc<dyn Fn() -> u64>,
    /// Marks the grid dirty and requests a frame.
    pub redraw: Rc<dyn Fn()>,
    pub set_cursor: Rc<dyn Fn(CursorKind)>,
    pub agent: Rc<dyn AgentView>,
    pub home: String,
    pub dbl: DoubleClick,
    /// The stat behind the path-link questions, wrapped once in a metered `StatGate`.
    pub stat: Arc<dyn StatFn>,
}

/// The front tab as M3's tab model keeps it. `cwd` is the tab's LOCAL folder after
/// `folder_after_report`, never a raw OSC 9 report. There is no shell id here on purpose: quoting
/// reads `handle.shell_id()`.
pub struct TabInfo<'a> {
    pub id: &'a str,
    pub handle: &'a Arc<TermHandle>,
    pub cwd: Option<&'a str>,
    pub alive: bool,
}

pub struct GridGeom {
    pub metrics: CellMetrics,
    pub scale: f32,
    pub cols: usize,
    pub rows: usize,
}

/// A pointer event over the grid. `kind`: 0 down, 1 up, 2 move, 3 cancel; `button`: 0 left,
/// 1 middle, 2 right, 3 other. Positions are CSS px from the grid area's top-left.
#[derive(Clone, Copy, Debug)]
pub struct PointerIn {
    pub kind: u8,
    pub button: u8,
    pub x_css: f64,
    pub y_css: f64,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Default)]
pub struct PointerOut {
    pub open_menu: Option<TermMenuAt>,
    pub focus_terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindBarState {
    pub open: bool,
    pub query: String,
    pub readout: String,
    pub can_step: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDone {
    Handled,
    Send,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickOut {
    pub redraw: bool,
    pub next_due_ms: Option<u64>,
    pub focus_terminal: bool,
}

/// What the frame decoration needs from the controller (shared with `LiveSource::decor`).
pub struct DecorInputs {
    pub hover: Option<(usize, usize)>,
    pub find_open: bool,
    pub ink: Option<DecorInk>,
    pub paths: PathLinks,
    pub ink_key: u32,
    pub owner: String,
    pub cwd: Option<String>,
    pub now: u64,
}

impl DecorInputs {
    fn new() -> Self {
        Self {
            hover: None,
            find_open: false,
            ink: None,
            paths: PathLinks::new(),
            ink_key: 0,
            owner: String::new(),
            cwd: None,
            now: 0,
        }
    }

    /// One frame's decoration for `handle`.
    pub fn decor(&mut self, handle: &TermHandle) -> Decor {
        let mut req = DecorReq {
            paths: &mut self.paths,
            owner: &self.owner,
            cwd: self.cwd.as_deref(),
            now_ms: self.now,
            ink_key: self.ink_key,
            hover: self.hover,
            find_open: self.find_open,
            ink: self.ink,
        };
        handle.decor(&mut req)
    }
}

/// The press that a release is judged against.
struct PressInfo {
    x_dev: f64,
    y_dev: f64,
    row: usize,
    b: usize,
    clicks: u32,
    mark: Option<LinkMark>,
}

struct FindUi {
    open: bool,
    tab: Option<String>,
    handle: Option<Arc<TermHandle>>,
    query: String,
    refresh_at: Option<u64>,
}

/// What the open menu was made from.
struct MenuStore {
    tab: String,
    x: f32,
    y: f32,
    selection: String,
    link: Option<String>,
    path: Option<TermPathAt>,
}

pub struct TermUi {
    deps: TermDeps,
    inputs: Rc<RefCell<DecorInputs>>,
    gate: Arc<StatGate>,
    machine: SelMachine,
    clicks: ClickCounter,
    badge: Rc<RefCell<BadgeClock>>,
    last_phase: BadgePhase,
    mouse_flag: Rc<Cell<bool>>,
    watch: AppCopyWatch<AppCopyAdapter>,
    watch_until: u64,
    held: Option<u8>,
    reporting: bool,
    reported_right: bool,
    reported_middle: bool,
    press: Option<PressInfo>,
    last_cell: Option<(usize, usize)>,
    last_cursor: Option<CursorKind>,
    last_drag_scroll: Option<u64>,
    find: FindUi,
    drops: DropBatcher,
    menu: Option<MenuStore>,
    last_rows: RefCell<Vec<TermMenuRow>>,
    ask_pending: bool,
    recheck: bool,
    stale: HashSet<String>,
}

fn mods(e: &PointerIn) -> MouseMods {
    MouseMods {
        shift: e.shift,
        alt: e.alt,
        ctrl: e.ctrl,
    }
}

impl TermUi {
    pub fn new(deps: TermDeps) -> Self {
        let badge = Rc::new(RefCell::new(BadgeClock::new()));
        let mouse_flag = Rc::new(Cell::new(false));
        let announce: Rc<dyn Fn()> = {
            let badge = Rc::clone(&badge);
            let clock = Rc::clone(&deps.clock_ms);
            let redraw = Rc::clone(&deps.redraw);
            Rc::new(move || {
                badge.borrow_mut().copied(clock());
                redraw();
            })
        };
        let watch = AppCopyWatch::new(AppCopyAdapter::new(
            Arc::clone(&deps.clipboard),
            Rc::clone(&mouse_flag),
            announce,
        ));
        let gate = Arc::new(StatGate::new(
            Arc::clone(&deps.stat),
            StatGateOptions::default(),
        ));
        let clicks = ClickCounter::new(deps.dbl.time_ms, deps.dbl.dx, deps.dbl.dy);
        Self {
            deps,
            inputs: Rc::new(RefCell::new(DecorInputs::new())),
            gate,
            machine: SelMachine::new(),
            clicks,
            badge,
            last_phase: BadgePhase::Gone,
            mouse_flag,
            watch,
            watch_until: 0,
            held: None,
            reporting: false,
            reported_right: false,
            reported_middle: false,
            press: None,
            last_cell: None,
            last_cursor: None,
            last_drag_scroll: None,
            find: FindUi {
                open: false,
                tab: None,
                handle: None,
                query: String::new(),
                refresh_at: None,
            },
            drops: DropBatcher::new(),
            menu: None,
            last_rows: RefCell::new(Vec::new()),
            ask_pending: false,
            recheck: false,
            stale: HashSet::new(),
        }
    }

    /// The decoration inputs `LiveSource::with_inputs` reads each frame.
    pub fn inputs(&self) -> Rc<RefCell<DecorInputs>> {
        Rc::clone(&self.inputs)
    }

    fn now(&self) -> u64 {
        (self.deps.clock_ms)()
    }

    fn redraw(&self) {
        (self.deps.redraw)();
    }

    fn sync_inputs(&self, tab: &TabInfo, now: u64) {
        let mut i = self.inputs.borrow_mut();
        if i.owner != tab.id {
            i.owner = tab.id.to_string();
        }
        if i.cwd.as_deref() != tab.cwd {
            i.cwd = tab.cwd.map(str::to_string);
        }
        i.now = now;
    }

    fn set_cursor(&mut self, k: CursorKind) {
        if self.last_cursor != Some(k) {
            self.last_cursor = Some(k);
            (self.deps.set_cursor)(k);
        }
    }

    fn set_hover(&mut self, h: Option<(usize, usize)>) -> bool {
        let mut i = self.inputs.borrow_mut();
        if i.hover == h {
            return false;
        }
        i.hover = h;
        true
    }

    /// A mouse report for the program. Not typing (rule 9).
    fn report(
        &self,
        tab: &TabInfo,
        e: &PointerIn,
        kind: MouseKind,
        button: Option<MouseButton>,
        cell: (usize, usize),
    ) {
        let m = tab.handle.mode();
        let evt = MouseEvt {
            kind,
            button,
            col: cell.0,
            row: cell.1,
            mods: mods(e),
        };
        if let Some(bytes) = encode_mouse(&evt, m.mouse, m.sgr_mouse) {
            tab.handle.write_report(&bytes);
        }
    }

    fn post_copied(&self) -> impl FnOnce(bool) + Send + 'static + use<> {
        let post = Arc::clone(&self.deps.post);
        move |ok| post(TermMsg::Copied(ok))
    }

    fn copy(&self, text: String, max: usize) {
        copy_async(
            Arc::clone(&self.deps.clipboard),
            text,
            max,
            self.post_copied(),
        );
    }

    fn read_clipboard(&self, tab: &str, mode: PasteMode) {
        let post = Arc::clone(&self.deps.post);
        read_async(
            Arc::clone(&self.deps.clipboard),
            PasteReq {
                tab: tab.to_string(),
                mode,
            },
            move |req, state| post(TermMsg::Clip(req, state)),
        );
    }

    /// The only way a path is opened: a worker re-resolves it and applies the allowlist.
    fn open_path_async(&self, base: String, text: String, mode: OpenMode) {
        let openers = Arc::clone(&self.deps.openers);
        let home = self.deps.home.clone();
        thread::spawn(move || {
            let _ = open_term_path(&base, &text, mode, &*openers, &home);
        });
    }

    fn open_link(&self, url: &str) {
        if is_web_url(url) {
            (self.deps.open_url)(url);
        }
    }

    // ---- the pointer ----

    /// One pointer event over the grid.
    pub fn on_pointer(&mut self, tab: Option<&TabInfo>, g: &GridGeom, e: PointerIn) -> PointerOut {
        let mut out = PointerOut::default();
        let Some(tab) = tab else {
            out.focus_terminal = e.kind == 0;
            return out;
        };
        let now = self.now();
        self.sync_inputs(tab, now);
        let scale = f64::from(g.scale);
        let dev = (e.x_css * scale, e.y_css * scale);
        let hit = hit_cell(dev.0, dev.1, &g.metrics, g.cols, g.rows);
        let row_c = hit.row.clamp(0, g.rows.max(1) as i64 - 1) as usize;
        let mode = tab.handle.mode();
        self.mouse_flag.set(mode.mouse != MouseTracking::None);
        let mouse_owned = mode.mouse != MouseTracking::None && !e.shift;
        match e.kind {
            0 => {
                out.focus_terminal = true;
                self.menu = None;
                self.last_rows.borrow_mut().clear();
                self.on_down(tab, &e, &hit, row_c, dev, mouse_owned, now);
            }
            1 => out.open_menu = self.on_up(tab, g, &e, &hit, row_c, dev, now),
            2 => self.on_move(tab, g, &e, &hit, row_c, dev.1, mouse_owned),
            _ => {
                self.machine.release();
                self.last_drag_scroll = None;
                self.held = None;
                self.reporting = false;
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn on_down(
        &mut self,
        tab: &TabInfo,
        e: &PointerIn,
        hit: &Hit,
        row_c: usize,
        dev: (f64, f64),
        mouse_owned: bool,
        now: u64,
    ) {
        let col = hit.col;
        self.held = Some(e.button);
        match e.button {
            0 => {
                let clicks = self.clicks.press(now, dev.0, dev.1, 0);
                self.watch.down(e.x_css, e.y_css, clicks, now);
                if mouse_owned {
                    self.reporting = true;
                    self.press = None;
                    self.last_cell = Some((col, row_c));
                    self.report(
                        tab,
                        e,
                        MouseKind::Press,
                        Some(MouseButton::Left),
                        (col, row_c),
                    );
                    return;
                }
                self.reporting = false;
                let has = tab.handle.with_emu_quiet(|em| em.has_selection());
                let machine = &mut self.machine;
                let (right, shift, alt) = (hit.right, e.shift, e.alt);
                tab.handle.with_emu(|em| {
                    let p = Press {
                        row: em.viewport_top() + row_c,
                        col,
                        right,
                        clicks,
                        shift,
                        alt,
                    };
                    let out = machine.press(&em.sel_env(), &p, has);
                    em.sel_apply(out);
                });
                let mark = tab.handle.with_emu_quiet(|em| em.link_at_view(row_c, col));
                self.press = Some(PressInfo {
                    x_dev: dev.0,
                    y_dev: dev.1,
                    row: row_c,
                    b: col + usize::from(hit.right),
                    clicks,
                    mark,
                });
                self.redraw();
            }
            2 => {
                let has = tab.handle.with_emu_quiet(|em| em.has_selection());
                if mouse_owned && !has {
                    self.reported_right = true;
                    self.report(
                        tab,
                        e,
                        MouseKind::Press,
                        Some(MouseButton::Right),
                        (col, row_c),
                    );
                }
            }
            1 if mouse_owned => {
                self.reported_middle = true;
                self.report(
                    tab,
                    e,
                    MouseKind::Press,
                    Some(MouseButton::Middle),
                    (col, row_c),
                );
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_move(
        &mut self,
        tab: &TabInfo,
        g: &GridGeom,
        e: &PointerIn,
        hit: &Hit,
        row_c: usize,
        y_dev: f64,
        mouse_owned: bool,
    ) {
        let col = hit.col;
        if self.held == Some(0) {
            if self.reporting {
                if self.last_cell != Some((col, row_c)) {
                    self.last_cell = Some((col, row_c));
                    self.report(
                        tab,
                        e,
                        MouseKind::Move,
                        Some(MouseButton::Left),
                        (col, row_c),
                    );
                }
            } else if self.press.is_some() {
                let past = past_edge_px(y_dev, &g.metrics, g.rows) / f64::from(g.scale).max(0.0001);
                let machine = &mut self.machine;
                let (right, row) = (hit.right, hit.row);
                tab.handle.with_emu(|em| {
                    let d = Drag {
                        row: em.viewport_top() as i64 + row,
                        col,
                        right,
                        past_css_px: past,
                    };
                    let out = machine.drag(&em.sel_env(), &d);
                    em.sel_apply(out);
                });
            }
            return;
        }
        let mark = if hit.inside {
            tab.handle.with_emu_quiet(|em| em.link_at_view(row_c, col))
        } else {
            None
        };
        let changed = self.set_hover(mark.as_ref().map(|_| (row_c, col)));
        let kind = if mark.is_some() {
            CursorKind::Pointer
        } else if mouse_owned {
            CursorKind::Default
        } else {
            CursorKind::Text
        };
        self.set_cursor(kind);
        if changed {
            self.redraw();
        }
        if mouse_owned && self.last_cell != Some((col, row_c)) {
            self.last_cell = Some((col, row_c));
            self.report(tab, e, MouseKind::Move, None, (col, row_c));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_up(
        &mut self,
        tab: &TabInfo,
        g: &GridGeom,
        e: &PointerIn,
        hit: &Hit,
        row_c: usize,
        dev: (f64, f64),
        now: u64,
    ) -> Option<TermMenuAt> {
        let col = hit.col;
        let was_held = self.held.take();
        match e.button {
            0 => {
                if was_held != Some(0) {
                    return None;
                }
                if self.reporting {
                    self.reporting = false;
                    self.report(
                        tab,
                        e,
                        MouseKind::Release,
                        Some(MouseButton::Left),
                        (col, row_c),
                    );
                    self.watch.up(e.x_css, e.y_css, now);
                    self.watch_until = now + 1100;
                    return None;
                }
                self.machine.release();
                self.last_drag_scroll = None;
                let has = tab.handle.with_emu_quiet(|em| em.has_selection());
                if !has {
                    tab.handle.with_emu(|em| em.sel_apply(SelOut::Clear));
                }
                let p = self.press.take()?;
                let dragged = (dev.0 - p.x_dev).hypot(dev.1 - p.y_dev)
                    > f64::from(g.metrics.cell_w) / 2.0
                    || has;
                if dragged {
                    return None;
                }
                let release = tab.handle.with_emu_quiet(|em| em.link_at_view(row_c, col));
                match (&p.mark, &release) {
                    (Some(a), Some(b)) if a == b => self.click_link(tab, a, now),
                    // A press and a release on different links open neither, and send no arrows.
                    (None, None) => {
                        let plan = tab.handle.with_emu_quiet(|em| {
                            em.click_caret_plan(
                                p.row,
                                p.b,
                                0,
                                p.clicks,
                                e.ctrl || e.shift || e.alt,
                                false,
                            )
                        });
                        if !plan.is_empty() {
                            tab.handle.send_text(&plan);
                        }
                    }
                    _ => {}
                }
                None
            }
            2 => {
                if self.reported_right {
                    self.reported_right = false;
                    self.report(
                        tab,
                        e,
                        MouseKind::Release,
                        Some(MouseButton::Right),
                        (col, row_c),
                    );
                }
                let ctx = {
                    let mut i = self.inputs.borrow_mut();
                    let paths = &mut i.paths;
                    tab.handle
                        .with_emu_quiet(|em| em.context_at(row_c, col, paths, tab.id, tab.cwd, now))
                };
                let store = MenuStore {
                    tab: tab.id.to_string(),
                    x: e.x_css as f32,
                    y: e.y_css as f32,
                    selection: ctx.selection,
                    link: ctx.link_text,
                    path: ctx.path,
                };
                let at = store.at();
                self.menu = Some(store);
                self.last_rows.borrow_mut().clear();
                Some(at)
            }
            1 => {
                if self.reported_middle {
                    self.reported_middle = false;
                    self.report(
                        tab,
                        e,
                        MouseKind::Release,
                        Some(MouseButton::Middle),
                        (col, row_c),
                    );
                }
                None
            }
            _ => None,
        }
    }

    /// A plain left click on a link mark. A web address opens through `open_url`; a path is
    /// handed to a worker that re-resolves it and applies the allowlist.
    fn click_link(&mut self, tab: &TabInfo, mark: &LinkMark, now: u64) {
        let shown = tab.handle.with_emu_quiet(|em| em.shown_for(mark));
        let base = match (&mark.target, tab.cwd) {
            (LinkTarget::Path { text }, Some(cwd)) => {
                let found = self.inputs.borrow().paths.find_path(tab.id, cwd, text, now);
                match found {
                    Found::Hit(f) => Some(f.base),
                    _ => None,
                }
            }
            _ => None,
        };
        match click_action(&mark.target, shown.as_deref(), base.as_deref(), 0) {
            LinkAction::OpenUrl(u) => self.open_link(&u),
            LinkAction::OpenPath { base, text } => {
                self.open_path_async(base, text, OpenMode::Open);
            }
            LinkAction::Nothing => {}
        }
    }

    // ---- keys ----

    /// Called for a key the chord table did not take, with `term_key_action`'s answer.
    pub fn on_key(&mut self, tab: &TabInfo, ev: &KeyEvent, action: TermKeyAction) -> KeyDone {
        match action {
            TermKeyAction::Copy => {
                let text = tab.handle.with_emu_quiet(|em| em.sel_text());
                tab.handle.with_emu(|em| em.sel_clear());
                self.copy(text, COPY_MAX);
                KeyDone::Handled
            }
            TermKeyAction::Deleted => KeyDone::Handled,
            TermKeyAction::Paste => {
                self.read_clipboard(tab.id, PasteMode::Normal);
                KeyDone::Handled
            }
            TermKeyAction::PastePlain => {
                self.read_clipboard(tab.id, PasteMode::Plain);
                KeyDone::Handled
            }
            TermKeyAction::Newline => {
                let agent = self.deps.agent.armed(tab.id);
                tab.handle.send_text(newline_key(agent));
                KeyDone::Handled
            }
            TermKeyAction::Host | TermKeyAction::Pty => {
                if ev.pressed {
                    let now = self.now();
                    if is_key(ev, 'c') && ev.ctrl && !ev.shift && !ev.alt && !ev.meta {
                        self.watch.copy_key(now);
                        self.watch_until = now + 1100;
                    } else {
                        self.watch.other_key();
                    }
                }
                KeyDone::Send
            }
        }
    }

    /// The closure for `KeyState::delete_selection`: asks the buffer for the plan, sends it,
    /// clears the selection. False = not allowed (the key is the shell's).
    pub fn selection_delete(&mut self, tab: &TabInfo) -> bool {
        let Some(keys) = tab.handle.with_emu_quiet(|em| em.selection_delete_plan()) else {
            return false;
        };
        if !keys.is_empty() {
            tab.handle.send_text(&keys);
        }
        tab.handle.with_emu(|em| em.sel_clear());
        true
    }

    pub fn has_selection(&self, tab: &TabInfo) -> bool {
        tab.handle.with_emu_quiet(|em| em.has_selection())
    }

    // ---- find ----

    /// Opens the bar for this tab, or closes it when it is already open here (D17).
    pub fn toggle_find(&mut self, tab: &TabInfo) {
        if self.find.open && self.find.tab.as_deref() == Some(tab.id) {
            self.find_close();
            return;
        }
        self.find_close();
        self.find.open = true;
        self.find.tab = Some(tab.id.to_string());
        self.find.handle = Some(Arc::clone(tab.handle));
        self.find.query.clear();
        self.inputs.borrow_mut().find_open = true;
        self.redraw();
    }

    fn apply_scroll(handle: &TermHandle, res: Option<(find::FindMatch, Option<i32>)>) {
        if let Some((_, Some(s))) = res {
            handle.scroll_view(-s);
        }
    }

    pub fn find_edited(&mut self, tab: &TabInfo, q: &str) {
        if !self.find.open || self.find.tab.as_deref() != Some(tab.id) {
            return;
        }
        self.find.query = q.to_string();
        let res = tab.handle.with_emu(|em| {
            em.find_set(q);
            if q.is_empty() {
                None
            } else {
                em.find_next(q, true)
            }
        });
        Self::apply_scroll(tab.handle, res);
    }

    pub fn find_next(&mut self, tab: &TabInfo) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let q = self.find.query.clone();
        let res = tab.handle.with_emu(|em| em.find_next(&q, false));
        Self::apply_scroll(tab.handle, res);
    }

    pub fn find_prev(&mut self, tab: &TabInfo) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let q = self.find.query.clone();
        let res = tab.handle.with_emu(|em| em.find_prev(&q));
        Self::apply_scroll(tab.handle, res);
    }

    /// Closing clears the highlights and keeps the selection.
    pub fn find_close(&mut self) {
        if let Some(h) = self.find.handle.take() {
            h.with_emu(|em| em.find_clear());
        }
        let was = self.find.open;
        self.find.open = false;
        self.find.tab = None;
        self.find.refresh_at = None;
        self.inputs.borrow_mut().find_open = false;
        if was {
            self.redraw();
        }
    }

    pub fn find_state(&self) -> FindBarState {
        let view = self
            .find
            .handle
            .as_ref()
            .map(|h| h.with_emu_quiet(|em| em.find_view()));
        let (count, index) = view.map_or((0, None), |v| (v.count, v.index));
        FindBarState {
            open: self.find.open,
            query: self.find.query.clone(),
            readout: find::readout(self.find.query.is_empty(), count, index),
            can_step: count > 0,
        }
    }

    // ---- events and messages ----

    /// An OSC 52 write from ANY tab. Nothing is ever written back to the program.
    pub fn on_term_event(&mut self, _tab: &str, ev: &TermEvent) {
        if let TermEvent::ClipboardWrite(t) = ev {
            self.copy(t.clone(), OSC52_MAX);
        }
    }

    /// Output arrived: the find re-runs 200 ms after the last of it, for the front tab.
    pub fn on_output(&mut self, tab: &str) {
        if self.find.open && self.find.tab.as_deref() == Some(tab) {
            self.find.refresh_at = Some(self.now() + 200);
        }
    }

    pub fn on_msg(&mut self, tab: Option<&TabInfo>, m: TermMsg) {
        let now = self.now();
        match m {
            TermMsg::Clip(req, state) => {
                let Some(t) = tab else { return };
                if t.id != req.tab || !t.alive {
                    return;
                }
                let shell = t.handle.shell_id().map(|s| path_shell_of(Some(s.as_str())));
                match paste_action(&state, req.mode, shell, self.deps.agent.armed(t.id)) {
                    PasteAction::ImageKey(k) => t.handle.send_text(k),
                    PasteAction::Text(text) => t.handle.paste(&text),
                    PasteAction::Nothing => {}
                }
            }
            TermMsg::Copied(ok) => {
                if ok {
                    self.badge.borrow_mut().copied(now);
                    self.redraw();
                }
            }
            TermMsg::PathAnswer(ask, hits) => {
                let woken = self.inputs.borrow_mut().paths.answer(&ask, hits, now);
                self.recheck = true;
                for o in woken {
                    match tab {
                        Some(t) if t.id == o => {
                            t.handle.with_emu(|em| em.links_revisit());
                            self.redraw();
                        }
                        _ => {
                            self.stale.insert(o);
                        }
                    }
                }
            }
        }
    }

    /// A file dropped on the terminal area. Buffered only while the front tab is live.
    pub fn on_dropped(&mut self, tab: Option<&TabInfo>, path: &Path) {
        if tab.is_some_and(|t| t.alive) {
            self.drops.push(path, self.now());
        }
    }

    // ---- the menu ----

    pub fn menu_rows(&self, help_on: bool, find_label: &str, help_label: &str) -> Vec<TermMenuRow> {
        let Some(m) = &self.menu else {
            return Vec::new();
        };
        let rows = term_menu_rows(&m.at(), help_on, find_label, help_label);
        *self.last_rows.borrow_mut() = rows.clone();
        rows
    }

    /// The pick acts on the tab the menu was opened on; when that tab is gone or not in front the
    /// pick does nothing.
    pub fn on_menu_pick(&mut self, tab: Option<&TabInfo>, i: usize) {
        let menu = self.menu.take();
        let rows = std::mem::take(&mut *self.last_rows.borrow_mut());
        let (Some(menu), Some(t)) = (menu, tab) else {
            return;
        };
        if t.id != menu.tab || !t.alive {
            return;
        }
        let Some(row) = rows.get(i) else { return };
        match row.act.clone() {
            TermMenuAct::OpenPath { path, reveal } => {
                let mode = if reveal {
                    OpenMode::Reveal
                } else {
                    OpenMode::Open
                };
                self.open_path_async(path.base, path.text, mode);
            }
            TermMenuAct::Copy(text) => self.copy(text, COPY_MAX),
            TermMenuAct::OpenLink(u) => self.open_link(&u),
            TermMenuAct::Paste => self.read_clipboard(t.id, PasteMode::Normal),
            TermMenuAct::Find => {
                if !(self.find.open && self.find.tab.as_deref() == Some(t.id)) {
                    self.toggle_find(t);
                }
            }
            TermMenuAct::Help => {}
        }
    }

    // ---- tabs ----

    /// The front tab changed: the menu and the find bar close (D17), a drag ends.
    pub fn tab_changed(&mut self, front: Option<&str>) {
        self.menu = None;
        self.last_rows.borrow_mut().clear();
        self.find_close();
        self.machine.release();
        self.last_drag_scroll = None;
        self.held = None;
        self.reporting = false;
        self.press = None;
        self.set_hover(None);
        if let Some(id) = front {
            self.inputs.borrow_mut().paths.subscribe(id);
        }
    }

    pub fn tab_closed(&mut self, id: &str) {
        {
            let mut i = self.inputs.borrow_mut();
            i.paths.forget_owner(id);
            i.paths.unsubscribe(id);
        }
        self.stale.remove(id);
        if self.find.tab.as_deref() == Some(id) {
            self.find_close();
        }
        if self.menu.as_ref().is_some_and(|m| m.tab == id) {
            self.menu = None;
        }
    }

    // ---- time ----

    pub fn badge_phase(&self) -> BadgePhase {
        self.badge.borrow().phase(self.now())
    }

    /// Everything that runs on the timer: the drag scroll, the badge, the program-copy looks, the
    /// find count and refresh, the path questions and the drop batch.
    pub fn tick(&mut self, tab: Option<&TabInfo>, _g: &GridGeom) -> TickOut {
        let now = self.now();
        let mut out = TickOut::default();
        let mut due: Vec<u64> = Vec::new();
        if let Some(t) = tab {
            self.sync_inputs(t, now);
            self.mouse_flag
                .set(t.handle.mode().mouse != MouseTracking::None);
        }

        let phase = self.badge.borrow().phase(now);
        if phase != self.last_phase {
            self.last_phase = phase;
            out.redraw = true;
        }
        if let Some(d) = self.badge.borrow().next_due(now) {
            due.push(d);
        }

        self.watch.advance(now);
        if now < self.watch_until {
            due.push(now + 50);
        }

        if let (true, Some(t)) = (self.machine.is_dragging(), tab) {
            if self
                .last_drag_scroll
                .is_none_or(|l| now >= l + DRAG_SCROLL_MS)
            {
                self.last_drag_scroll = Some(now);
                let lines = self.machine.scroll_lines();
                if lines != 0 {
                    t.handle.scroll_view(-lines);
                    let machine = &mut self.machine;
                    t.handle.with_emu(|em| {
                        let view = ViewBox {
                            top: em.viewport_top(),
                            rows: em.view_rows(),
                        };
                        let o = machine.after_scroll(&em.sel_env(), &view);
                        em.sel_apply(o);
                    });
                    out.redraw = true;
                }
            }
            due.push(self.last_drag_scroll.unwrap_or(now) + DRAG_SCROLL_MS);
        }

        if self.find.open
            && let Some(h) = self.find.handle.clone()
        {
            if !h.with_emu_quiet(|em| em.find_view().done) {
                let t0 = Instant::now();
                let clock = move || t0.elapsed().as_secs_f64() * 1000.0;
                h.with_emu(|em| em.find_step(&clock));
                out.redraw = true;
                due.push(now + 16);
            }
            if let Some(at) = self.find.refresh_at {
                if now >= at {
                    h.with_emu(|em| em.find_refresh());
                    self.find.refresh_at = None;
                    out.redraw = true;
                } else {
                    due.push(at);
                }
            }
        }

        if let Some(t) = tab {
            self.pump_paths(t, now, &mut out);
        }
        if self.ask_pending || self.recheck {
            due.push(now + BATCH_MS);
            self.recheck = false;
        }

        if let Some(paths) = self.drops.take_ready(now) {
            if let Some(t) = tab.filter(|t| t.alive) {
                type_dropped(t.handle, &paths);
                out.focus_terminal = true;
            }
        } else if self.drops.is_pending() {
            due.push(now + DROP_QUIET_MS);
        }

        if out.redraw {
            self.redraw();
        }
        out.next_due_ms = due.into_iter().min();
        out
    }

    /// The pump M2 left for the host: moves queued questions along and starts one metered worker.
    fn pump_paths(&mut self, t: &TabInfo, now: u64, out: &mut TickOut) {
        let woken = self.inputs.borrow_mut().paths.advance(now);
        for o in woken {
            if o == t.id {
                t.handle.with_emu(|em| em.links_revisit());
                out.redraw = true;
            } else {
                self.stale.insert(o);
            }
        }
        if self.stale.remove(t.id) {
            t.handle.with_emu(|em| em.links_revisit());
            out.redraw = true;
        }
        if self.inputs.borrow_mut().paths.take_asked(t.id) {
            self.ask_pending = true;
        }
        let ask = self.inputs.borrow_mut().paths.take_ask();
        if let Some(ask) = ask {
            self.ask_pending = false;
            self.spawn_ask(ask);
        }
    }

    fn spawn_ask(&self, ask: Ask) {
        let gate = Arc::clone(&self.gate);
        let home = self.deps.home.clone();
        let post = Arc::clone(&self.deps.post);
        let post_fail = Arc::clone(&self.deps.post);
        let ask_fail = ask.clone();
        let spawned = thread::Builder::new().spawn(move || {
            let hits = catch_unwind(AssertUnwindSafe(|| {
                path_kinds(&ask.base, &ask.paths, &gate, &home)
            }))
            .ok();
            post(TermMsg::PathAnswer(ask, hits));
        });
        if spawned.is_err() {
            post_fail(TermMsg::PathAnswer(ask_fail, None));
        }
    }
}

impl MenuStore {
    fn at(&self) -> TermMenuAt {
        TermMenuAt {
            tab: self.tab.clone(),
            x: self.x,
            y: self.y,
            selection: self.selection.clone(),
            link: self.link.clone(),
            path: self.path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openers::{OpenedRecord, RecordingOpeners};
    use flare_agent::kinds::AgentKind;
    use flare_render::glyph::{FakeGlyphs, GlyphSource};
    use flare_render::hit::SCALES;
    use flare_render::metrics::cell_metrics;
    use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
    use flare_term::handle::StartOptions;
    use flare_term::keys::{KeyState, term_key_action};
    use flare_term::path_open::FsStat;
    use flare_term::session::{SessionManager, SessionOptions};
    use flare_win::clipboard_port::MemoryClipboard;
    use flare_win::dblclick::FALLBACK;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::thread::ThreadId;
    use std::time::Duration;

    const COLS: usize = 30;
    const ROWS: usize = 6;

    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if f() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    struct TestAgent(Cell<Option<AgentKind>>);
    impl AgentView for TestAgent {
        fn armed(&self, _tab: &str) -> Option<AgentKind> {
            self.0.get()
        }
    }

    /// A clipboard that refuses every write.
    struct Refusing;
    impl ClipboardPort for Refusing {
        fn write_text(&self, _t: &str, _m: usize) -> Result<(), String> {
            Err("busy".to_string())
        }
        fn read_text(&self) -> Option<String> {
            None
        }
        fn read_state(&self) -> ClipboardState {
            ClipboardState::default()
        }
        fn sequence(&self) -> u32 {
            0
        }
    }

    /// Records the thread of every call.
    #[derive(Default)]
    struct Probe {
        inner: MemoryClipboard,
        threads: Mutex<Vec<ThreadId>>,
    }
    impl Probe {
        fn note(&self) {
            self.threads.lock().unwrap().push(thread::current().id());
        }
    }
    impl ClipboardPort for Probe {
        fn write_text(&self, t: &str, m: usize) -> Result<(), String> {
            self.note();
            self.inner.write_text(t, m)
        }
        fn read_text(&self) -> Option<String> {
            self.note();
            self.inner.read_text()
        }
        fn read_state(&self) -> ClipboardState {
            self.note();
            self.inner.read_state()
        }
        fn sequence(&self) -> u32 {
            self.inner.sequence()
        }
    }

    struct Rig {
        ui: TermUi,
        handle: Arc<TermHandle>,
        fake: Arc<FakeState>,
        clip: Arc<MemoryClipboard>,
        openers: Arc<RecordingOpeners>,
        urls: Rc<RefCell<Vec<String>>>,
        msgs: Arc<Mutex<VecDeque<TermMsg>>>,
        clock: Rc<Cell<u64>>,
        cursors: Rc<RefCell<Vec<CursorKind>>>,
        agent: Rc<TestAgent>,
        g: GridGeom,
        cwd: Option<String>,
        _sp: Arc<FakeSpawner>,
    }

    impl Rig {
        fn new(scale: f32) -> Rig {
            let clip = Arc::new(MemoryClipboard::new());
            Rig::build(scale, clip.clone(), clip, None)
        }

        fn with_cwd(scale: f32, cwd: &Path) -> Rig {
            let clip = Arc::new(MemoryClipboard::new());
            Rig::build(
                scale,
                clip.clone(),
                clip,
                Some(cwd.to_string_lossy().to_string()),
            )
        }

        fn with_port(scale: f32, port: Arc<dyn ClipboardPort>) -> Rig {
            Rig::build(scale, port, Arc::new(MemoryClipboard::new()), None)
        }

        fn build(
            scale: f32,
            port: Arc<dyn ClipboardPort>,
            clip: Arc<MemoryClipboard>,
            cwd: Option<String>,
        ) -> Rig {
            let sp = FakeSpawner::new();
            let mgr = SessionManager::new(sp.clone(), FakeShells::new(), SessionOptions::default());
            let handle = TermHandle::new("a", mgr, COLS, ROWS);
            handle.start(StartOptions {
                root: "C:\\".into(),
                ..StartOptions::default()
            });
            until("the spawn", || !sp.made().is_empty());
            let mut started = false;
            until("Started", || {
                started |= handle.drain_events().contains(&TermEvent::Started);
                started
            });
            let fake = sp.made()[0].clone();
            let openers = Arc::new(RecordingOpeners::new());
            let urls = Rc::new(RefCell::new(Vec::new()));
            let msgs: Arc<Mutex<VecDeque<TermMsg>>> = Arc::default();
            let clock = Rc::new(Cell::new(0u64));
            let cursors = Rc::new(RefCell::new(Vec::new()));
            let agent = Rc::new(TestAgent(Cell::new(None)));
            let deps = TermDeps {
                clipboard: port,
                openers: openers.clone(),
                open_url: {
                    let u = Rc::clone(&urls);
                    Rc::new(move |s| u.borrow_mut().push(s.to_string()))
                },
                post: {
                    let m = Arc::clone(&msgs);
                    Arc::new(move |t| m.lock().unwrap().push_back(t))
                },
                clock_ms: {
                    let c = Rc::clone(&clock);
                    Rc::new(move || c.get())
                },
                redraw: Rc::new(|| {}),
                set_cursor: {
                    let c = Rc::clone(&cursors);
                    Rc::new(move |k| c.borrow_mut().push(k))
                },
                agent: agent.clone(),
                home: std::env::temp_dir().to_string_lossy().to_string(),
                dbl: FALLBACK,
                stat: Arc::new(FsStat),
            };
            let metrics = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), scale);
            let mut rig = Rig {
                ui: TermUi::new(deps),
                handle,
                fake,
                clip,
                openers,
                urls,
                msgs,
                clock,
                cursors,
                agent,
                g: GridGeom {
                    metrics,
                    scale,
                    cols: COLS,
                    rows: ROWS,
                },
                cwd,
                _sp: sp,
            };
            rig.ui.tab_changed(Some("a"));
            rig.tick(0);
            rig
        }

        fn feed(&self, s: &str) {
            self.handle.with_emu(|e| {
                e.feed(s.as_bytes());
            });
        }

        fn text(&self) -> String {
            self.handle.with_emu_quiet(|e| e.sel_text())
        }

        fn has_sel(&self) -> bool {
            self.handle.with_emu_quiet(|e| e.has_selection())
        }

        fn top(&self) -> usize {
            self.handle.with_emu_quiet(|e| e.viewport_top())
        }

        fn written(&self) -> Vec<u8> {
            self.fake.written()
        }

        fn wait_written(&self, n: usize) -> Vec<u8> {
            until("pty bytes", || self.fake.written().len() >= n);
            self.fake.written()
        }

        /// A pointer position in CSS px for a cell (`right`: right half) at this rig's scale.
        fn css(&self, col: usize, row: usize, right: bool) -> (f64, f64) {
            let m = &self.g.metrics;
            let s = f64::from(self.g.scale);
            let x = f64::from(m.padding)
                + col as f64 * f64::from(m.cell_w)
                + if right { 0.75 } else { 0.25 } * f64::from(m.cell_w);
            let y =
                f64::from(m.padding) + row as f64 * f64::from(m.cell_h) + f64::from(m.cell_h) / 2.0;
            (x / s, y / s)
        }

        fn ptr_at(
            &mut self,
            kind: u8,
            button: u8,
            xy: (f64, f64),
            shift: bool,
            alt: bool,
        ) -> PointerOut {
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            let e = PointerIn {
                kind,
                button,
                x_css: xy.0,
                y_css: xy.1,
                ctrl: false,
                shift,
                alt,
            };
            self.ui.on_pointer(Some(&tab), &self.g, e)
        }

        fn down(&mut self, col: usize, row: usize, right: bool) -> PointerOut {
            let p = self.css(col, row, right);
            self.ptr_at(0, 0, p, false, false)
        }
        fn mv(&mut self, col: usize, row: usize, right: bool) -> PointerOut {
            let p = self.css(col, row, right);
            self.ptr_at(2, 0, p, false, false)
        }
        fn up(&mut self, col: usize, row: usize, right: bool) -> PointerOut {
            let p = self.css(col, row, right);
            self.ptr_at(1, 0, p, false, false)
        }
        fn hover(&mut self, col: usize, row: usize) {
            let p = self.css(col, row, false);
            self.ptr_at(2, 0, p, false, false);
        }
        fn right_click(&mut self, col: usize, row: usize) -> PointerOut {
            let p = self.css(col, row, false);
            self.ptr_at(0, 2, p, false, false);
            self.ptr_at(1, 2, p, false, false)
        }
        fn click(&mut self, col: usize, row: usize) {
            self.down(col, row, false);
            self.up(col, row, false);
        }
        /// Selects the boundaries `a..b` of `row` by a drag.
        fn drag(&mut self, row: usize, a: usize, b: usize) {
            self.down(a, row, false);
            self.mv(b, row, false);
            self.up(b, row, false);
        }

        fn tick(&mut self, advance: u64) -> TickOut {
            self.clock.set(self.clock.get() + advance);
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            self.ui.tick(Some(&tab), &self.g)
        }

        fn frame(&self) -> Decor {
            self.ui.inputs().borrow_mut().decor(&self.handle)
        }

        fn key(&mut self, ev: KeyEvent) -> (TermKeyAction, KeyDone) {
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            let has = self.ui.has_selection(&tab);
            let agent_here = self.agent.armed("a").is_some();
            let action = {
                let ui = &mut self.ui;
                let mut del = || ui.selection_delete(&tab);
                let host = |_: &KeyEvent| false;
                let mut st = KeyState {
                    has_selection: has,
                    delete_selection: &mut del,
                    host_owns: &host,
                    agent_here,
                };
                term_key_action(&ev, &mut st)
            };
            let done = self.ui.on_key(&tab, &ev, action);
            (action, done)
        }

        fn pump_for(&mut self, tab_id: &str) {
            until("a message", || !self.msgs.lock().unwrap().is_empty());
            // Let a possible second message land, then drain.
            thread::sleep(Duration::from_millis(20));
            let all: Vec<TermMsg> = self.msgs.lock().unwrap().drain(..).collect();
            let h = self.handle.clone();
            let tab = TabInfo {
                id: tab_id,
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            for m in all {
                self.ui.on_msg(Some(&tab), m);
            }
        }

        fn pump(&mut self) {
            self.pump_for("a");
        }

        fn find_edited(&mut self, q: &str) {
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            self.ui.find_edited(&tab, q);
        }

        fn with_tab<R>(&mut self, f: impl FnOnce(&mut TermUi, &TabInfo) -> R) -> R {
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                alive: true,
            };
            f(&mut self.ui, &tab)
        }

        /// Ticks until the find count has finished.
        fn settle_find(&mut self) {
            for _ in 0..3000 {
                if self.handle.with_emu_quiet(|e| e.find_view().done) {
                    return;
                }
                self.tick(10);
            }
            panic!("the find count never finished");
        }

        fn badge(&self) -> BadgePhase {
            self.ui.badge_phase()
        }
    }

    fn ctrl(k: &str, code: &str) -> KeyEvent {
        KeyEvent::new(k, code).ctrl()
    }

    // ---- selection ----

    #[test]
    fn a_drag_selects_between_the_boundaries_it_crossed() {
        for s in SCALES {
            let mut r = Rig::new(s);
            r.feed("hello world");
            r.drag(0, 0, 5);
            assert_eq!(r.text(), "hello", "scale {s}");
        }
    }

    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        for s in SCALES {
            let mut r = Rig::new(s);
            r.feed("hello world");
            r.click(3, 0);
            assert!(!r.has_sel(), "scale {s}");
        }
    }

    #[test]
    fn a_double_click_selects_the_word_and_a_triple_the_line() {
        let mut r = Rig::new(1.5);
        r.feed("git status");
        r.click(1, 0);
        r.clock.set(100);
        r.click(1, 0);
        assert_eq!(r.text(), "git");
        r.clock.set(200);
        r.click(1, 0);
        assert_eq!(r.text(), "git status");
    }

    #[test]
    fn slow_clicks_do_not_count_as_a_double() {
        let mut r = Rig::new(1.0);
        r.feed("git status");
        r.click(1, 0);
        r.clock.set(600);
        r.click(1, 0);
        assert!(!r.has_sel());
    }

    #[test]
    fn shift_click_extends_the_selection() {
        let mut r = Rig::new(2.25);
        r.feed("hello world");
        r.drag(0, 0, 5);
        assert_eq!(r.text(), "hello");
        let p = r.css(8, 0, false);
        r.ptr_at(0, 0, p, true, false);
        r.ptr_at(1, 0, p, true, false);
        assert_eq!(r.text(), "hello wo");
    }

    #[test]
    fn alt_drag_selects_a_block() {
        let mut r = Rig::new(1.0);
        r.feed("abcdef\r\nghijkl");
        let a = r.css(2, 0, false);
        let b = r.css(4, 1, false);
        r.ptr_at(0, 0, a, false, true);
        r.ptr_at(2, 0, b, false, true);
        r.ptr_at(1, 0, b, false, true);
        assert_eq!(r.text(), "cd\r\nij");
    }

    #[test]
    fn dragging_below_the_screen_scrolls_the_view_and_extends() {
        for s in [1.0f32, 2.25] {
            let mut r = Rig::new(s);
            let lines: String = (0..50).map(|i| format!("l{i}\r\n")).collect();
            r.feed(&lines);
            r.handle.scroll_view(1000);
            assert_eq!(r.top(), 0);
            r.down(0, 0, false);
            let m = r.g.metrics;
            let y_dev = f64::from(m.padding) + (ROWS as f64) * f64::from(m.cell_h) + 100.0;
            let x = r.css(2, 0, false).0;
            r.ptr_at(2, 0, (x, y_dev / f64::from(s)), false, false);
            for _ in 0..3 {
                r.tick(50);
            }
            assert!(r.top() > 0, "the view scrolled at {s}");
            let end = r
                .handle
                .with_emu_quiet(|e| e.sel_bounds())
                .expect("a selection")
                .1;
            assert_eq!(
                end.0,
                r.top() + ROWS - 1,
                "the end follows the last visible row"
            );
        }
    }

    // ---- copy ----

    #[test]
    fn ctrl_c_with_a_selection_copies_exactly_and_clears() {
        let mut r = Rig::new(1.0);
        r.feed("hello world");
        r.drag(0, 0, 5);
        let (a, d) = r.key(ctrl("c", "KeyC"));
        assert_eq!((a, d), (TermKeyAction::Copy, KeyDone::Handled));
        r.pump();
        assert_eq!(r.clip.read_text().as_deref(), Some("hello"));
        assert!(!r.has_sel());
        assert_eq!(r.badge(), BadgePhase::Shown);
        assert!(!r.written().contains(&3));
    }

    #[test]
    fn ctrl_c_without_a_selection_is_the_interrupt() {
        let mut r = Rig::new(1.0);
        let (a, d) = r.key(ctrl("c", "KeyC"));
        assert_eq!((a, d), (TermKeyAction::Pty, KeyDone::Send));
    }

    #[test]
    fn copying_nothing_or_too_much_raises_no_badge() {
        // A selection of blanks writes nothing.
        let mut r = Rig::new(1.0);
        r.drag(0, 0, 3);
        r.key(ctrl("c", "KeyC"));
        r.pump();
        assert_eq!(r.clip.read_text(), None);
        assert_eq!(r.badge(), BadgePhase::Gone);
        // A clipboard that refuses.
        let mut r = Rig::with_port(1.0, Arc::new(Refusing));
        r.feed("hello");
        r.drag(0, 0, 5);
        r.key(ctrl("c", "KeyC"));
        r.pump();
        assert_eq!(r.badge(), BadgePhase::Gone);
    }

    #[test]
    fn backspace_over_a_selection_deletes_through_the_shell_keys() {
        let mut r = Rig::new(1.0);
        r.feed("echo hello");
        r.drag(0, 7, 10);
        assert_eq!(r.text(), "llo");
        let (a, d) = r.key(KeyEvent::new("Backspace", "Backspace"));
        assert_eq!((a, d), (TermKeyAction::Deleted, KeyDone::Handled));
        assert_eq!(r.wait_written(3), b"\x7f\x7f\x7f");
        assert!(!r.has_sel());
        assert!(r.handle.last_typed().is_some());
    }

    #[test]
    fn backspace_with_no_plan_goes_to_the_shell() {
        let mut r = Rig::new(1.0);
        r.feed("older line\r\nPS> abc");
        r.drag(0, 0, 5);
        let (a, d) = r.key(KeyEvent::new("Backspace", "Backspace"));
        assert_eq!((a, d), (TermKeyAction::Pty, KeyDone::Send));
        assert!(r.has_sel());
    }

    #[test]
    fn a_plain_click_on_the_prompt_line_sends_arrows() {
        let mut r = Rig::new(1.75);
        r.feed("PS> abc");
        r.click(5, 0);
        assert_eq!(r.wait_written(6), b"\x1b[D\x1b[D");
        assert!(r.handle.last_typed().is_some());
    }

    // ---- links ----

    #[test]
    fn a_click_on_a_link_opens_it_once_and_sends_no_arrows() {
        let mut r = Rig::new(1.0);
        r.feed("see https://a.io/x ok");
        r.frame();
        r.click(8, 0);
        assert_eq!(*r.urls.borrow(), vec!["https://a.io/x".to_string()]);
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty());
    }

    #[test]
    fn a_right_click_on_a_link_opens_nothing_and_offers_the_menu() {
        let mut r = Rig::new(1.0);
        r.feed("see https://a.io/x ok");
        r.frame();
        let out = r.right_click(8, 0);
        assert!(r.urls.borrow().is_empty());
        let m = out.open_menu.expect("a menu");
        assert_eq!(m.link.as_deref(), Some("https://a.io/x"));
    }

    #[test]
    fn a_press_and_release_on_different_links_opens_neither() {
        let mut r = Rig::new(1.0);
        r.feed("https://a.io https://b.io");
        r.frame();
        r.down(3, 0, false);
        r.up(16, 0, false);
        assert!(r.urls.borrow().is_empty());
    }

    #[test]
    fn hover_sets_the_pointer_cursor_over_a_link_and_text_elsewhere() {
        let mut r = Rig::new(1.0);
        r.feed("see https://a.io/x ok");
        r.frame();
        r.hover(8, 0);
        r.hover(1, 0);
        assert_eq!(
            *r.cursors.borrow(),
            vec![CursorKind::Pointer, CursorKind::Text]
        );
        r.hover(8, 0);
        assert_eq!(r.ui.inputs().borrow().hover, Some((0, 8)));
    }

    /// Prints the texts, asks the worker and handles its answer, so the paths light up.
    fn light_up(r: &mut Rig, text: &str) {
        r.feed(text);
        r.tick(0);
        let first = r.frame();
        assert!(first.links.is_empty(), "no span before the worker answers");
        r.tick(40);
        r.pump();
    }

    #[test]
    fn a_printed_path_lights_up_after_the_worker_answers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.txt\r\nb.txt");
        let d = r.frame();
        assert!(
            d.links
                .iter()
                .any(|s| s.row == 0 && s.col == 0 && s.width == 5)
        );
        assert!(
            !d.links.iter().any(|s| s.row == 1),
            "a missing file never lights up"
        );
    }

    #[test]
    fn a_path_link_click_opens_through_the_allowlist_on_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join("run.bat"), "x").unwrap();
        std::fs::create_dir(dir.path().join("x.d")).unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.txt run.bat x.d");
        r.frame();
        r.click(2, 0);
        r.click(8, 0);
        r.click(15, 0);
        until("three opener calls", || r.openers.records().len() >= 3);
        // Three workers run at once, so the order is theirs.
        let rec = r.openers.records();
        let has = |f: &dyn Fn(&OpenedRecord) -> bool| rec.iter().any(f);
        assert!(
            has(&|x| matches!(x, OpenedRecord::Open(p) if p.ends_with("a.txt"))),
            "{rec:?}"
        );
        assert!(
            has(&|x| matches!(x, OpenedRecord::Reveal(p) if p.ends_with("run.bat"))),
            "{rec:?}"
        );
        assert!(
            has(&|x| matches!(x, OpenedRecord::Folder(p) if p.ends_with("x.d"))),
            "{rec:?}"
        );
        assert!(!has(
            &|x| matches!(x, OpenedRecord::Open(p) if p.ends_with("run.bat"))
        ));
    }

    #[test]
    fn a_path_menu_pick_re_resolves_and_never_opens_abs_directly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let base = dir.path().to_string_lossy().to_string();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.ui.menu = Some(MenuStore {
            tab: "a".into(),
            x: 0.0,
            y: 0.0,
            selection: String::new(),
            link: None,
            path: Some(TermPathAt {
                kind: flare_term::path_open::PathKind::File,
                abs: "C:\\evil.exe".into(),
                text: "a.txt".into(),
                base,
            }),
        });
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let i = rows
            .iter()
            .position(|x| x.label == "Open")
            .expect("an Open row");
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), i));
        until("the opener", || !r.openers.records().is_empty());
        let rec = r.openers.records();
        assert!(
            matches!(&rec[0], OpenedRecord::Open(p) if p.ends_with("a.txt") && !p.contains("evil")),
            "{rec:?}"
        );
    }

    #[test]
    fn a_closed_tab_forgets_what_it_taught() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        let inputs = r.ui.inputs();
        {
            // The tab shows a path in a folder that exists, so it teaches that folder.
            let mut i = inputs.borrow_mut();
            let hit = |p: &str| {
                Some(PathHit {
                    kind: flare_term::path_open::PathKind::File,
                    abs: p.to_string(),
                })
            };
            i.paths
                .link_ranges("see C:\\d\\f.txt", Some("C:\\cwd"), "a", 0);
            i.paths.advance(50);
            let ask = i.paths.take_ask().expect("an ask");
            let hits = ask.paths.iter().map(|p| hit(p)).collect();
            i.paths.answer(&ask, Some(hits), 50);
            i.paths
                .link_ranges("see C:\\d\\f.txt and rel\\x.txt", Some("C:\\cwd"), "a", 60);
        }
        assert_eq!(inputs.borrow().paths.owners_kept(), 1);
        r.ui.tab_closed("a");
        assert_eq!(inputs.borrow().paths.owners_kept(), 0);
    }

    // ---- mouse ownership ----

    const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1006h";

    #[test]
    fn a_program_that_owns_the_mouse_gets_press_release_and_motion() {
        let mut r = Rig::new(1.0);
        r.feed(MOUSE_ON);
        r.down(2, 1, false);
        r.mv(3, 1, false);
        r.mv(3, 1, true);
        r.up(3, 1, false);
        let want = b"\x1b[<0;3;2M\x1b[<32;4;2M\x1b[<0;4;2m";
        assert_eq!(r.wait_written(want.len()), want);
        assert!(!r.has_sel());
        assert!(r.handle.last_typed().is_none());
    }

    #[test]
    fn shift_gives_the_selection_back_in_mouse_mode() {
        let mut r = Rig::new(1.0);
        r.feed("hello world");
        r.feed(MOUSE_ON);
        let a = r.css(0, 0, false);
        let b = r.css(5, 0, false);
        r.ptr_at(0, 0, a, true, false);
        r.ptr_at(2, 0, b, true, false);
        r.ptr_at(1, 0, b, true, false);
        assert_eq!(r.text(), "hello");
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty());
    }

    #[test]
    fn a_right_press_in_mouse_mode_is_reported_unless_a_selection_stands() {
        let mut r = Rig::new(1.0);
        r.feed("hello world");
        r.feed(MOUSE_ON);
        let out = r.right_click(2, 0);
        assert!(out.open_menu.is_some());
        let want = b"\x1b[<2;3;1M\x1b[<2;3;1m";
        assert_eq!(r.wait_written(want.len()), want);
        // A standing selection (made with shift) keeps the press from the program.
        let a = r.css(0, 0, false);
        let b = r.css(5, 0, false);
        r.ptr_at(0, 0, a, true, false);
        r.ptr_at(2, 0, b, true, false);
        r.ptr_at(1, 0, b, true, false);
        let n = r.written().len();
        let out = r.right_click(2, 0);
        assert!(out.open_menu.is_some());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(r.written().len(), n);
    }

    // ---- the menu ----

    #[test]
    fn the_menu_opens_on_the_right_release_with_the_context() {
        let mut r = Rig::new(1.0);
        r.feed("hello see https://a.io/x");
        r.drag(0, 0, 5);
        let out = r.right_click(1, 0);
        let m = out.open_menu.expect("a menu");
        assert_eq!(m.selection, "hello");
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        assert_eq!(rows[0].label, "Copy");
        r.frame();
        r.click(19, 0);
        let out = r.right_click(19, 0);
        assert!(out.open_menu.is_some());
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        assert_eq!(rows[0].label, "Copy link");
    }

    #[test]
    fn a_menu_pick_acts_once() {
        let mut r = Rig::new(1.0);
        r.feed("see https://a.io/x ok");
        r.frame();
        r.right_click(8, 0);
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let copy = rows.iter().position(|x| x.label == "Copy link").unwrap();
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), copy));
        r.pump();
        assert_eq!(r.clip.read_text().as_deref(), Some("https://a.io/x"));
        assert_eq!(r.badge(), BadgePhase::Shown);
        // The menu is gone: the same pick again does nothing.
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), copy));
        assert!(r.msgs.lock().unwrap().is_empty());

        r.right_click(8, 0);
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let open = rows.iter().position(|x| x.label == "Open link").unwrap();
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), open));
        assert_eq!(*r.urls.borrow(), vec!["https://a.io/x".to_string()]);

        r.right_click(1, 0);
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let paste = rows.iter().position(|x| x.label == "Paste").unwrap();
        r.clip.set_text("pasted");
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), paste));
        until("one read", || r.msgs.lock().unwrap().len() == 1);
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), paste));
        thread::sleep(Duration::from_millis(30));
        assert_eq!(r.msgs.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_menu_pick_for_another_tab_does_nothing() {
        let mut r = Rig::new(1.0);
        r.feed("hello");
        r.drag(0, 0, 5);
        r.right_click(1, 0);
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let h = r.handle.clone();
        let other = TabInfo {
            id: "b",
            handle: &h,
            cwd: None,
            alive: true,
        };
        assert_eq!(rows[0].label, "Copy");
        r.ui.on_menu_pick(Some(&other), 0);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(r.clip.read_text(), None);
    }

    #[test]
    fn tab_changed_puts_the_menu_and_find_away() {
        let mut r = Rig::new(1.0);
        r.feed("hello");
        r.right_click(1, 0);
        r.with_tab(|ui, t| ui.toggle_find(t));
        assert!(r.ui.find_state().open);
        r.ui.tab_changed(Some("b"));
        assert!(r.ui.menu_rows(false, "Ctrl+F", "").is_empty());
        assert!(!r.ui.find_state().open);
    }

    // ---- paste ----

    #[test]
    fn ctrl_v_reads_the_clipboard_on_a_worker_and_pastes_once() {
        let mut r = Rig::new(1.0);
        r.feed("\x1b[?2004h");
        r.clip.set_text("a\nb");
        let (a, d) = r.key(ctrl("v", "KeyV"));
        assert_eq!((a, d), (TermKeyAction::Paste, KeyDone::Handled));
        r.pump();
        let want = b"\x1b[200~a\rb\x1b[201~";
        assert_eq!(r.wait_written(want.len()), want);
        // The request that lands after the front tab changed pastes nothing.
        r.key(ctrl("v", "KeyV"));
        r.pump_for("b");
        thread::sleep(Duration::from_millis(30));
        assert_eq!(r.written(), want);
    }

    #[test]
    fn ctrl_shift_v_pastes_plain_text_even_with_an_image() {
        let mut r = Rig::new(1.0);
        r.clip.set_text("plain");
        r.clip.set_image(true);
        let (a, _) = r.key(ctrl("v", "KeyV").shift());
        assert_eq!(a, TermKeyAction::PastePlain);
        r.pump();
        assert_eq!(r.wait_written(5), b"plain");
    }

    #[test]
    fn an_image_sends_the_agent_key_once() {
        let mut r = Rig::new(1.0);
        r.clip.set_image(true);
        r.key(ctrl("v", "KeyV"));
        r.pump();
        assert_eq!(r.wait_written(1), b"\x16");
        assert!(r.handle.last_typed().is_some());
        r.agent.0.set(Some(AgentKind::Claude));
        r.key(ctrl("v", "KeyV"));
        r.pump();
        assert_eq!(r.wait_written(3), b"\x16\x1bv");
    }

    #[test]
    fn a_hostile_paste_is_inert() {
        let mut r = Rig::new(1.0);
        r.feed("\x1b[?2004h");
        r.clip.set_text("x\u{1b}[201~calc\r");
        r.key(ctrl("v", "KeyV"));
        r.pump();
        thread::sleep(Duration::from_millis(30));
        let log = r.written();
        let end = b"\x1b[201~";
        let hits = log.windows(end.len()).filter(|w| *w == end).count();
        assert_eq!(hits, 1, "{log:?}");
        assert!(log.ends_with(end));
    }

    #[test]
    fn shift_enter_where_an_agent_runs_sends_the_newline_key() {
        let mut r = Rig::new(1.0);
        let (a, d) = r.key(KeyEvent::new("Enter", "Enter").shift());
        assert_eq!((a, d), (TermKeyAction::Pty, KeyDone::Send));
        r.agent.0.set(Some(AgentKind::Claude));
        let (a, d) = r.key(KeyEvent::new("Enter", "Enter").shift());
        assert_eq!((a, d), (TermKeyAction::Newline, KeyDone::Handled));
        assert_eq!(r.wait_written(1), b"\n");
        r.agent.0.set(Some(AgentKind::Codex));
        r.key(KeyEvent::new("Enter", "Enter").shift());
        assert_eq!(r.wait_written(3), b"\n\\\r");
    }

    // ---- find ----

    #[test]
    fn find_types_incrementally_and_enter_steps_and_wraps() {
        let mut r = Rig::new(1.0);
        r.feed("foo x foo x foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.find_edited("f");
        r.find_edited("fo");
        r.find_edited("foo");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1 of 3");
        let mut seen = Vec::new();
        for _ in 0..3 {
            r.with_tab(|ui, t| ui.find_next(t));
            seen.push(r.ui.find_state().readout);
        }
        assert_eq!(seen, ["2 of 3", "3 of 3", "1 of 3"]);
        r.with_tab(|ui, t| ui.find_prev(t));
        assert_eq!(r.ui.find_state().readout, "3 of 3");
    }

    #[test]
    fn find_scrolls_only_when_the_match_is_outside_the_view() {
        let mut r = Rig::new(1.0);
        let lines: String = (0..40).map(|_| "x\r\n").collect();
        r.feed(&format!("needle\r\n{lines}foo"));
        r.with_tab(|ui, t| ui.toggle_find(t));
        let before = r.top();
        assert!(before > 0);
        r.find_edited("foo");
        assert_eq!(r.top(), before, "a match in view does not scroll");
        r.find_edited("needle");
        assert!(r.top() < before, "a match outside the view scrolls");
        assert!(r.top() == 0);
    }

    #[test]
    fn find_readout_and_buttons_follow_the_count() {
        let mut r = Rig::new(1.0);
        r.feed("foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        let s = r.ui.find_state();
        assert_eq!((s.readout.as_str(), s.can_step), ("", false));
        r.find_edited("zzz");
        r.settle_find();
        let s = r.ui.find_state();
        assert_eq!((s.readout.as_str(), s.can_step), ("none", false));
        r.find_edited("foo");
        r.settle_find();
        let s = r.ui.find_state();
        assert_eq!((s.readout.as_str(), s.can_step), ("1 of 1", true));
    }

    #[test]
    fn a_big_count_reads_a_thousand_plus() {
        let mut r = Rig::new(1.0);
        let lines: String = (0..1500).map(|_| "foo\r\n").collect();
        r.feed(&lines);
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.find_edited("foo");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1 of 1000");
        // The last match lies beyond the thousand that are counted.
        r.with_tab(|ui, t| ui.find_prev(t));
        assert_eq!(r.ui.find_state().readout, "1000+");
    }

    #[test]
    fn closing_find_clears_highlights_and_keeps_the_selection() {
        let mut r = Rig::new(1.0);
        r.feed("foo x foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.find_edited("foo");
        r.settle_find();
        let d = r.frame();
        assert!(!d.find.is_empty() || !d.find_active.is_empty());
        r.ui.find_close();
        let d = r.frame();
        assert!(d.find.is_empty() && d.find_active.is_empty());
        assert!(r.has_sel(), "the selection stays");
        assert!(!r.ui.find_state().open);
    }

    #[test]
    fn find_belongs_to_one_tab() {
        let mut r = Rig::new(1.0);
        r.with_tab(|ui, t| ui.toggle_find(t));
        assert!(r.ui.find_state().open);
        r.ui.tab_changed(Some("other"));
        assert!(!r.ui.find_state().open);
    }

    #[test]
    fn toggling_find_twice_closes_it() {
        let mut r = Rig::new(1.0);
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.with_tab(|ui, t| ui.toggle_find(t));
        assert!(!r.ui.find_state().open);
    }

    #[test]
    fn output_while_find_is_open_refreshes_after_200_ms_without_scrolling() {
        let mut r = Rig::new(1.0);
        r.feed("foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.find_edited("foo");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1 of 1");
        let top = r.top();
        r.feed("\r\nfoo");
        r.ui.on_output("a");
        r.tick(100);
        assert_eq!(r.ui.find_state().readout, "1 of 1", "not yet");
        r.tick(120);
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1 of 2");
        assert_eq!(r.top(), top);
    }

    // ---- drop ----

    #[test]
    fn a_dropped_file_types_its_quoted_path_after_the_quiet_time_and_never_enter() {
        let mut r = Rig::new(1.0);
        let h = r.handle.clone();
        let tab = TabInfo {
            id: "a",
            handle: &h,
            cwd: None,
            alive: true,
        };
        r.ui.on_dropped(Some(&tab), Path::new("C:\\My Dir\\a.txt"));
        let t = r.tick(10);
        assert!(!t.focus_terminal);
        assert!(t.next_due_ms.is_some());
        assert!(r.written().is_empty());
        let t = r.tick(50);
        assert!(t.focus_terminal);
        let want = b"'C:\\My Dir\\a.txt'";
        assert_eq!(r.wait_written(want.len()), want);
        assert!(!r.written().contains(&b'\r'));
    }

    #[test]
    fn a_drop_over_a_dead_or_missing_tab_types_nothing() {
        let mut r = Rig::new(1.0);
        let h = r.handle.clone();
        let dead = TabInfo {
            id: "a",
            handle: &h,
            cwd: None,
            alive: false,
        };
        r.ui.on_dropped(Some(&dead), Path::new("C:\\a.txt"));
        r.ui.on_dropped(None, Path::new("C:\\b.txt"));
        r.tick(100);
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty());
    }

    // ---- OSC 52 and the program's own copy ----

    #[test]
    fn osc_52_from_a_background_tab_copies_and_badges() {
        let mut r = Rig::new(1.0);
        r.ui.on_term_event("b", &TermEvent::ClipboardWrite("OSC52 \u{e6}".into()));
        r.pump();
        assert_eq!(r.clip.read_text().as_deref(), Some("OSC52 \u{e6}"));
        assert_eq!(r.badge(), BadgePhase::Shown);
        assert!(r.written().is_empty(), "nothing is written back");
    }

    #[test]
    fn osc_52_failure_raises_no_badge() {
        let mut r = Rig::with_port(1.0, Arc::new(Refusing));
        r.ui.on_term_event("a", &TermEvent::ClipboardWrite("x".into()));
        r.pump();
        assert_eq!(r.badge(), BadgePhase::Gone);
    }

    #[test]
    fn the_program_copy_is_announced_after_a_selecting_release_in_mouse_mode() {
        let mut r = Rig::new(1.0);
        r.feed(MOUSE_ON);
        r.clip.set_text("before");
        r.down(0, 0, false);
        r.mv(5, 0, false);
        r.up(5, 0, false);
        r.clip.set_text("copied by the program");
        for _ in 0..40 {
            r.tick(100);
            if r.badge() == BadgePhase::Shown {
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(r.badge(), BadgePhase::Shown);
    }

    #[test]
    fn the_badge_follows_its_clock() {
        let mut r = Rig::new(1.0);
        r.ui.on_msg(None, TermMsg::Copied(true));
        assert_eq!(r.badge(), BadgePhase::Shown);
        let t = r.tick(1200);
        assert_eq!(r.badge(), BadgePhase::Leaving);
        assert!(t.redraw);
        r.tick(200);
        assert_eq!(r.badge(), BadgePhase::Gone);
    }

    #[test]
    fn no_blocking_clipboard_call_on_the_ui_thread() {
        let probe = Arc::new(Probe::default());
        let mut r = Rig::with_port(1.0, probe.clone());
        r.feed("hello");
        r.drag(0, 0, 5);
        r.key(ctrl("c", "KeyC"));
        r.pump();
        probe.inner.set_text("paste me");
        r.key(ctrl("v", "KeyV"));
        r.pump();
        r.ui.on_term_event("a", &TermEvent::ClipboardWrite("x".into()));
        r.pump();
        let me = thread::current().id();
        let seen = probe.threads.lock().unwrap().clone();
        assert!(seen.len() >= 3);
        assert!(
            seen.iter().all(|t| *t != me),
            "a clipboard call ran on the test thread"
        );
    }
}
