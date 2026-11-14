//! The one controller of the terminal's selection, copy, paste, find and links (Task 23).
//!
//! TermUi holds no Slint types. It is driven by plain structs (a pointer event, a key, a tick) and
//! calls the closures in `TermDeps`, so every behaviour is tested without a window or a shell.
//! Positions arrive in CSS px relative to the grid area; every hit test is in whole device pixels.
//! Source: PT `TerminalPanel.tsx` (`attachClickCaret`, `deleteSelection`, `termContextAt`, the key
//! handler, `pasteHere`), `App.tsx` (the menu, the drop, the find chord), xterm `SelectionService`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use flare_agent::keys::{AgentView, newline_key, word_rubout_key};
use flare_render::decor::{Decor, DecorInk};
use flare_render::hit::{Hit, hit_cell, past_edge_px};
use flare_render::metrics::CellMetrics;
use flare_shells::paste::{ClipboardState, path_shell_of};
use flare_tabs::tip_place::BoxF;
use flare_term::alt_click::ALT_CLICK_MS;
use flare_term::app_copy::AppCopyWatch;
use flare_term::click_caret::MouseTracking;
use flare_term::emulator::{DecorReq, TermEvent};
use flare_term::find;
use flare_term::handle::TermHandle;
use flare_term::keys::{KeyEvent, KeyOs, TermKeyAction, is_key};
use flare_term::link_scan::{LinkMark, LinkTarget};
use flare_term::link_target::{LinkAction, TermPathAt, click_action};
use flare_term::mouse_report::{MouseButton, MouseEvt, MouseKind, MouseMods, encode_mouse};
use flare_term::osc52::OSC52_MAX;
use flare_term::path_open::{
    OpenMode, Opened, PathKind, PathOpeners, ext_label, first_existing, open_term_path,
    resolve_openable_file, resolve_term_path,
};
use flare_term::prompt_keys::{self as pk, Highlight, Input, PromptKey, PromptSel};
use flare_term::sel_machine::{
    ClickCounter, DRAG_SCROLL_MS, Drag, Press, SelMachine, SelOut, ViewBox,
};
use flare_term::web_url::is_web_url;
use flare_win::clipboard_port::{COPY_MAX, ClipboardPort};
use flare_win::dblclick::DoubleClick;

use crate::badge::{BadgeClock, BadgePhase};
use crate::clip_flow::{
    AppCopyAdapter, LatestCopier, PasteAction, PasteMode, PasteReq, copy_async, is_small_copy,
    paste_action, read_async, remote_upload,
};
use crate::drop_flow::{DROP_QUIET_MS, DropBatcher, type_dropped};
use crate::term_menu::{TermMenuAct, TermMenuAt, TermMenuRow, term_menu_rows};
use crate::thumbs::{Thumb, ThumbHover, ThumbJob, ThumbView, ThumbWorker, item_box};

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
    /// A big copy's answer: the tab and copy number it began with, and `Some(ok)` when the copy
    /// ran (`None` when it was dropped). The UI frees that tab's pending copy when it handles it.
    BigCopied(String, u64, Option<bool>),
    /// A click resolved to a file the panel may show: the tab, what to show it as, and the path.
    Preview {
        tab: String,
        kind: flare_surfaces::PanelKind,
        abs: String,
        /// The line the link named: the text view scrolls to it and marks it.
        line: Option<u32>,
        /// The person asked outright ("Show in side panel" in the menu): the "Open previews in Flare" setting,
        /// which only decides what a plain click does, does not stop it.
        asked: bool,
    },
    /// "Show in side panel" was refused when the worker looked again (the file changed, went away, or is not
    /// one the panel may show): the app tells the person in the panel. Nothing is ever opened in Windows.
    PreviewRefused {
        tab: String,
    },
    /// A click on a file link whose path (in every spelling the link allows) names nothing: the app says so, where a
    /// refused preview says it.
    NoSuchFile {
        tab: String,
        text: String,
    },
    /// A hover thumbnail's answer: the job's number and the picture, None for a refused, abandoned or broken one.
    Thumb {
        seq: u64,
        thumb: Option<Arc<Thumb>>,
    },
    /// A click on a file link in a remote tab (remote sessions PR 6, rule 303): its spellings, which name a file of
    /// the host and never one of this machine. The app has the tab's link fetch it.
    RemoteLink {
        tab: String,
        texts: Vec<String>,
        line: Option<u32>,
    },
    /// Files dropped on a remote tab, or copied files or an image pasted there (rule 304): the app copies them to
    /// the host first and pastes their host paths. `files` are this machine's paths; `image` is the clipboard's image.
    RemoteUpload {
        tab: String,
        files: Vec<String>,
        image: bool,
    },
    /// A remote tab's file work did not get done (the link is down, the host refused, the file is too big): one line
    /// for the person, said where a refused preview is.
    RemoteFileRefused {
        tab: String,
        text: String,
    },
}

/// One `path-click` crumb for a file link's click or menu Open (owner, 2026-11-13: a click that opened nothing left
/// no trace). `how` is `click`, `menu-open` or `menu-reveal`; `route` is where it went: `panel` (the side panel),
/// `opened` (handed to Windows' default app; a failure there is its own `open-failed` crumb), `revealed`
/// (Explorer, selected), `missing` (no spelling names anything) or `refused` (it changed under the click). The
/// extension is a fixed word (`ext_label`): the screen text never reaches the log.
fn link_crumb(how: &str, route: &str, text: &str, started: Instant) {
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    crate::diag_ctl::crumb(
        "path-click",
        &[
            ("how", crate::diag_ctl::txt(how)),
            ("route", crate::diag_ctl::txt(route)),
            ("ext", crate::diag_ctl::txt(ext_label(text))),
            ("ms", serde_json::Value::from(ms)),
        ],
    );
}

/// `open_term_path`'s answer as a `path-click` route.
fn route_of(got: Option<Opened>) -> &'static str {
    match got {
        Some(Opened::Opened) => "opened",
        Some(Opened::Revealed) => "revealed",
        None => "refused",
    }
}

/// Puts the hover thumbnail on the window (Some), or takes it down (None).
pub type SetThumb = Rc<dyn Fn(Option<&ThumbView>)>;

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
    /// Puts the hover thumbnail on the window, or takes it down.
    pub set_thumb: SetThumb,
    pub agent: Rc<dyn AgentView>,
    pub home: String,
    pub dbl: DoubleClick,
    /// The opt-in "Send right-clicks to programs" setting, read on every right press. Off (the
    /// default): a right-click opens Flare's menu and sends nothing to the program.
    pub right_click_reports: Rc<dyn Fn() -> bool>,
}

/// The front tab as M3's tab model keeps it. `cwd` is the folder a relative file link is read from: the folder
/// the tab's agent works in while its hooks have named one that a relative path can be read from (changes after
/// review, 2026-11-11), else the tab's LOCAL folder after `folder_after_report`, never a raw OSC 9 report.
/// `shell_cwd` is the tab's own folder, which a rooted path with no drive (`\x`) and `~` are read from; None reads
/// them from `cwd` too. There is no shell id here on purpose: quoting reads `handle.shell_id()`.
pub struct TabInfo<'a> {
    pub id: &'a str,
    pub handle: &'a Arc<TermHandle>,
    pub cwd: Option<&'a str>,
    pub shell_cwd: Option<&'a str>,
    pub alive: bool,
    /// A remote tab (remote sessions PR 3): its file links name files of the host, never of this machine, so none is
    /// resolved, opened, previewed or offered here (a click asks the tab's link to fetch it, PR 6), and a file dropped
    /// on it is not typed (its path is this machine's; the app copies it to the host first, PR 6).
    pub remote: bool,
}

impl TabInfo<'_> {
    /// The folder a file link spelled `text` is read from: the agent's folder only for a relative path (rule 275).
    fn link_base(&self, text: &str) -> &str {
        if flare_term::path_grammar::is_relative(text) {
            self.cwd.unwrap_or_default()
        } else {
            self.shell_cwd.or(self.cwd).unwrap_or_default()
        }
    }
}

pub struct GridGeom {
    pub metrics: CellMetrics,
    pub scale: f32,
    pub cols: usize,
    pub rows: usize,
    /// The window's size in device pixels (0, 0 with no window). Only a hover thumbnail reads it: any change takes it down.
    pub window: (u32, u32),
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

thread_local! {
    /// The link colour the worn theme chose (`AppliedLook::link_colour`), set by the look apply.
    static WORN_LINK: Cell<Option<flare_render::color::Rgba>> = const { Cell::new(None) };
}

thread_local! {
    /// The find highlight colours the worn theme chose (`AppliedLook::find`), set by the look apply.
    static WORN_FIND: Cell<Option<[flare_render::color::Rgba; 4]>> = const { Cell::new(None) };
}

/// Records the theme's find colours (match, its line, the active match, its line); junk is ignored.
pub fn set_worn_find(find: &flare_theme::find_ink::FindInk) {
    use flare_render::color::Rgba;
    let all =
        [&find.bg, &find.border, &find.active_bg, &find.active_border].map(|h| Rgba::from_hex(h));
    if let [Some(a), Some(b), Some(c), Some(d)] = all {
        WORN_FIND.with(|w| w.set(Some([a, b, c, d].map(|x| Rgba::rgb(x.r, x.g, x.b)))));
    }
}

/// Records the theme's link colour for the decoration (a hex the theme computed; junk is ignored).
pub fn set_worn_link(hex: &str) {
    if let Some(c) = flare_render::color::Rgba::from_hex(hex) {
        WORN_LINK.with(|w| w.set(Some(c)));
    }
}

/// The default ink with the worn theme's link colour, or `None` before any theme was worn.
fn worn_ink() -> Option<DecorInk> {
    WORN_LINK.with(Cell::get).map(|link| {
        let mut ink = DecorInk {
            link: flare_render::color::Rgba::rgb(link.r, link.g, link.b),
            ..DecorInk::pt_default()
        };
        if let Some([bg, border, active_bg, active_border]) = WORN_FIND.with(Cell::get) {
            ink.find_bg = bg;
            ink.find_border = border;
            ink.active_bg = active_bg;
            ink.active_border = active_border;
        }
        ink
    })
}

/// The ink the decoration paints with: the controller's explicit ink, else the worn theme's. The
/// e2e probes read this too, so they report the colour the links really wear.
pub fn effective_ink(ink: Option<DecorInk>) -> Option<DecorInk> {
    ink.or_else(worn_ink)
}

/// What the frame decoration needs from the controller (shared with `LiveSource::decor`).
pub struct DecorInputs {
    pub hover: Option<(usize, usize)>,
    pub find_open: bool,
    pub ink: Option<DecorInk>,
    pub ink_key: u32,
    pub cwd: Option<String>,
}

impl DecorInputs {
    fn new() -> Self {
        Self {
            hover: None,
            find_open: false,
            ink: None,
            ink_key: 0,
            cwd: None,
        }
    }

    /// One frame's decoration for `handle`.
    pub fn decor(&mut self, handle: &TermHandle) -> Decor {
        let mut req = DecorReq {
            cwd: self.cwd.as_deref(),
            ink_key: self.ink_key,
            hover: self.hover,
            find_open: self.find_open,
            ink: effective_ink(self.ink),
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
    /// When the button went down (ms), for the Alt+click hold limit.
    at: u64,
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
    /// Counts the big copies started; a finished read whose number is no longer the last is dropped.
    copy_gen: Arc<AtomicU64>,
    /// The tabs with a big copy in flight, each with the number of that copy and when it began:
    /// Ctrl+C on such a tab is still a copy until the answer. Per tab (audit 2026-10-20, L49), so
    /// one tab's copy never turns another tab's Ctrl+C into a copy, and a closed tab or a copy that
    /// never answers (`COPY_PENDING_MAX_MS`) cannot hold it for good.
    copy_pending: Arc<Mutex<HashMap<String, (u64, Instant)>>>,
    /// The one worker for OSC 52 writes: latest wins.
    osc_copier: LatestCopier,
    /// "Open previews in Flare" (Surfaces stage 1): a click on a path to a picture or a text file
    /// asks the app to show it in the tab's panel. Set from the setting on every refresh.
    previews: bool,
    /// "Show thumbnails when hovering image paths": Some only while the setting is on.
    thumbs: Option<ThumbState>,
    /// Flare's selection over an agent's prompt, and the tab it was made on (rule 1235).
    prompt_sel: Option<(String, PromptSel)>,
}

/// How often the prompt's selection is looked at while it stands: should the agent's repaint drop the highlight
/// (the selection guard covers the usual erase), the tick puts it back (rule 1235).
const PROMPT_SEL_RECHECK_MS: u64 = 120;

/// The longest a big copy may keep Ctrl+C a copy.
const COPY_PENDING_MAX_MS: u64 = 30_000;

/// How often a hover thumbnail looks again at the link under its cell (the app's timer).
const THUMB_RECHECK_MS: u64 = 50;

/// "The same link in the same place": the tab, the first row on the screen and the mark. A scroll is a change even
/// when a long wrapped link is still under the cell, because the picture was placed for the old rows.
#[derive(Clone, PartialEq)]
struct ThumbAt {
    tab: String,
    top: usize,
    mark: LinkMark,
}

/// What a thumbnail was placed for; any change takes it down (a resize, a DPI change, a font change).
type ThumbGeom = (u32, u32, u32, u32, usize, usize, (u32, u32));

fn thumb_geom(g: &GridGeom) -> ThumbGeom {
    (
        g.scale.to_bits(),
        g.metrics.cell_w,
        g.metrics.cell_h,
        g.metrics.padding,
        g.cols,
        g.rows,
        g.window,
    )
}

/// The hover thumbnails' state. There is none at all while the setting is off.
struct ThumbState {
    hover: ThumbHover<ThumbAt>,
    worker: ThumbWorker,
    /// The cell the pointer was last on: (row on the screen, column).
    cell: (usize, usize),
    geom: ThumbGeom,
    /// Where the path was when the rest ended, in css px from the grid's corner.
    item: Option<BoxF>,
}

/// The image link at a cell now, from the marks of the last frame.
fn thumb_at(tab: &TabInfo, cell: (usize, usize)) -> Option<ThumbAt> {
    if tab.remote {
        return None;
    }
    let (mark, top) = tab
        .handle
        .with_emu_quiet(|em| (em.link_at_view(cell.0, cell.1), em.viewport_top()));
    thumb_of(tab, mark?, top)
}

/// Only a path whose name is a picture's. A web address never is: hovering must not reach the network.
fn thumb_of(tab: &TabInfo, mark: LinkMark, top: usize) -> Option<ThumbAt> {
    let LinkTarget::Path { text, alts, .. } = &mark.target else {
        return None;
    };
    let image =
        |t: &String| flare_surfaces::preview::classify(t) == Some(flare_surfaces::PanelKind::Image);
    (image(text) || alts.iter().any(image)).then(|| ThumbAt {
        tab: tab.id.to_string(),
        top,
        mark,
    })
}

/// What the panel may show `text` as, and the file as the file system names it: a picture or a text file on the click
/// allowlist (rules 13 and 14). Runs on a worker (one stat).
pub(crate) fn previewable(
    base: &str,
    text: &str,
    home: &str,
) -> Option<(flare_surfaces::PanelKind, String)> {
    let (abs, real) = resolve_openable_file(base, text, home)?;
    let kind = flare_surfaces::preview::classify_resolved(&abs, &real)?;
    flare_surfaces::preview::loadable(&real, &kind).then_some((kind, real))
}

/// The menu's path for a file link (a path between marks): read from the tab's folder, a folder when the text ends in a
/// separator. Nothing is statted: the row a person picks is vetted on a worker.
fn marked_path_at(target: &LinkTarget, tab: &TabInfo, home: &str) -> Option<TermPathAt> {
    let LinkTarget::Path {
        text, line, alts, ..
    } = target
    else {
        return None;
    };
    if tab.remote {
        return None;
    }
    let base = tab.link_base(text).to_string();
    let kind = if text.ends_with(['\\', '/']) {
        PathKind::Dir
    } else {
        PathKind::File
    };
    Some(TermPathAt {
        kind,
        abs: resolve_term_path(&base, text, home).unwrap_or_else(|| text.clone()),
        text: text.clone(),
        base,
        line: *line,
        alts: alts.clone(),
    })
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
        let osc_copier = LatestCopier::new(Arc::clone(&deps.clipboard));

        let clicks = ClickCounter::new(deps.dbl.time_ms, deps.dbl.dx, deps.dbl.dy);
        Self {
            deps,
            inputs: Rc::new(RefCell::new(DecorInputs::new())),
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
            copy_gen: Arc::new(AtomicU64::new(0)),
            copy_pending: Arc::new(Mutex::new(HashMap::new())),
            osc_copier,
            previews: true,
            thumbs: None,
            prompt_sel: None,
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

    fn sync_inputs(&self, tab: &TabInfo) {
        let mut i = self.inputs.borrow_mut();
        if i.cwd.as_deref() != tab.cwd {
            i.cwd = tab.cwd.map(str::to_string);
        }
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

    /// Ctrl+C with a selection. A small one is read and cleared here, as before. A big one is read
    /// on a worker in chunks and written there; its selection stands until the copy is read, and
    /// `copy_pending` keeps Ctrl+C a copy (not the interrupt) until the UI has the answer. A newer
    /// copy drops the older one's answer.
    fn copy_selection(&self, tab: &TabInfo) {
        let rows = tab.handle.with_emu_quiet(|em| em.sel_row_count());
        if is_small_copy(rows) {
            let text = tab.handle.with_emu_quiet(|em| em.sel_text());
            tab.handle.with_emu(|em| em.sel_clear());
            self.copy(text, COPY_MAX);
            return;
        }
        let mine = self.copy_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.copy_pending
            .lock()
            .unwrap()
            .insert(tab.id.to_string(), (mine, Instant::now()));
        let tab_id = tab.id.to_string();
        let latest = Arc::clone(&self.copy_gen);
        let post = Arc::clone(&self.deps.post);
        let clip = Arc::clone(&self.deps.clipboard);
        let handle = Arc::clone(tab.handle);
        tab.handle.selection_text_async(Box::new(move |text| {
            if latest.load(Ordering::SeqCst) != mine {
                post(TermMsg::BigCopied(tab_id, mine, None));
                return;
            }
            let Some(text) = text else {
                // The selection changed while it was read: nothing is copied and no badge shows.
                post(TermMsg::BigCopied(tab_id, mine, Some(false)));
                return;
            };
            handle.with_emu(|em| em.sel_clear());
            copy_async(clip, text, COPY_MAX, move |ok| {
                post(TermMsg::BigCopied(tab_id, mine, Some(ok)))
            });
        }));
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

    /// A plain click on a file link (a path between marks). A worker takes the first spelling that names something
    /// (`texts`: as written, then with a space where a row broke) and re-resolves it (rule 13). With previews on, a
    /// picture or a text file on the click allowlist (rule 14) goes to the tab's panel, at `line`; everything else (a
    /// folder, a document, a script) goes the way it always went. A path that names nothing is said in the panel.
    /// Never runs anything.
    fn click_path_async(&self, tab: String, base: String, texts: Vec<String>, line: Option<u32>) {
        let openers = Arc::clone(&self.deps.openers);
        let home = self.deps.home.clone();
        let post = Arc::clone(&self.deps.post);
        let previews = self.previews;
        let started = Instant::now();
        thread::spawn(move || {
            let Some(text) = first_existing(&base, &texts, &home) else {
                let text = texts.into_iter().next().unwrap_or_default();
                link_crumb("click", "missing", &text, started);
                post(TermMsg::NoSuchFile { tab, text });
                return;
            };
            if previews && let Some((kind, abs)) = previewable(&base, &text, &home) {
                link_crumb("click", "panel", &text, started);
                post(TermMsg::Preview {
                    tab,
                    kind,
                    abs,
                    line,
                    asked: false,
                });
                return;
            }
            let got = open_term_path(&base, &text, OpenMode::Open, &*openers, &home);
            link_crumb("click", route_of(got), &text, started);
        });
    }

    /// "Show in side panel" from the path menu: the same vetting as the click, and when the path is not a picture or
    /// a text file after all (it changed since the menu was built) the panel says so: this row never opens anything.
    /// A link none of whose spellings names anything is said as the click says it.
    fn preview_path_async(&self, tab: String, base: String, texts: Vec<String>, line: Option<u32>) {
        let home = self.deps.home.clone();
        let post = Arc::clone(&self.deps.post);
        thread::spawn(move || {
            let Some(text) = first_existing(&base, &texts, &home) else {
                let text = texts.into_iter().next().unwrap_or_default();
                post(TermMsg::NoSuchFile { tab, text });
                return;
            };
            match previewable(&base, &text, &home) {
                Some((kind, abs)) => post(TermMsg::Preview {
                    tab,
                    kind,
                    abs,
                    line,
                    asked: true,
                }),
                None => post(TermMsg::PreviewRefused { tab }),
            }
        });
    }

    /// "Copy path" for a file link: the first spelling that names something, read from the tab's folder, as a whole
    /// path. When none does, the text as written (a table cell's path without its borders), never a guess.
    fn copy_path_async(&self, base: String, texts: Vec<String>) {
        let home = self.deps.home.clone();
        let clip = Arc::clone(&self.deps.clipboard);
        let done = self.post_copied();
        thread::spawn(move || {
            let copied = match first_existing(&base, &texts, &home) {
                Some(text) => resolve_term_path(&base, &text, &home).unwrap_or(text),
                None => texts.into_iter().next().unwrap_or_default(),
            };
            copy_async(clip, copied, COPY_MAX, done);
        });
    }

    /// Whether a click on a picture or a text file shows it in the panel.
    pub fn set_previews(&mut self, on: bool) {
        self.previews = on;
    }

    /// "Show thumbnails when hovering image paths". Off drops the state, the memory and any worker's claim on them.
    pub fn set_thumbs(&mut self, on: bool) {
        if on == self.thumbs.is_some() {
            return;
        }
        if !on {
            if let Some(mut st) = self.thumbs.take() {
                st.worker.want(None);
                if st.hover.shown() {
                    (self.deps.set_thumb)(None);
                }
            }
            return;
        }
        let post = Arc::clone(&self.deps.post);
        self.thumbs = Some(ThumbState {
            hover: ThumbHover::new(),
            worker: ThumbWorker::new(
                self.deps.home.clone(),
                Arc::new(move |seq, thumb| post(TermMsg::Thumb { seq, thumb })),
            ),
            cell: (0, 0),
            geom: (0, 0, 0, 0, 0, 0, (0, 0)),
            item: None,
        });
    }

    /// The pointer stands on `cell` of the grid, over `mark` if any (`None`: over no link or off the grid).
    fn thumb_point(
        &mut self,
        tab: &TabInfo,
        g: &GridGeom,
        cell: (usize, usize),
        mark: Option<LinkMark>,
    ) {
        let now = self.now();
        let Some(st) = self.thumbs.as_mut() else {
            return;
        };
        let at = mark.and_then(|m| {
            let top = tab.handle.with_emu_quiet(|em| em.viewport_top());
            thumb_of(tab, m, top)
        });
        if at.is_some() {
            st.cell = cell;
        }
        let p = st.hover.point(at, now);
        if p.started {
            st.geom = thumb_geom(g);
        }
        st.worker.want(st.hover.loading_seq());
        if p.came_down {
            (self.deps.set_thumb)(None);
        }
    }

    /// The pointer left the grid.
    pub fn pointer_left(&mut self) {
        let now = self.now();
        let Some(st) = self.thumbs.as_mut() else {
            return;
        };
        let came_down = st.hover.point(None, now).came_down;
        st.worker.want(st.hover.loading_seq());
        if came_down {
            (self.deps.set_thumb)(None);
        }
    }

    /// A key, a press, a menu or Find: the thumbnail goes, and this link stays quiet until the pointer has left it.
    pub fn hide_thumb(&mut self) {
        let Some(st) = self.thumbs.as_mut() else {
            return;
        };
        let came_down = st.hover.dismiss();
        st.worker.want(st.hover.loading_seq());
        if came_down {
            (self.deps.set_thumb)(None);
        }
    }

    /// Every tick while a thumbnail is waiting, loading or up: asks what is under the cell NOW. A stored key cannot say
    /// that the screen scrolled under a pointer that never moved.
    fn thumb_tick(&mut self, tab: Option<&TabInfo>, g: &GridGeom, now: u64, due: &mut Vec<u64>) {
        let Some(st) = self.thumbs.as_mut() else {
            return;
        };
        if !st.hover.active() {
            return;
        }
        let mut came_down = false;
        if st.geom != thumb_geom(g) {
            came_down |= st.hover.dismiss();
        }
        let here = tab.and_then(|t| thumb_at(t, st.cell));
        came_down |= st.hover.recheck(here.as_ref());
        if let Some(seq) = st.hover.fire(now) {
            let job = here.as_ref().zip(tab).and_then(|(at, t)| {
                let LinkTarget::Path { text, alts, .. } = &at.mark.target else {
                    return None;
                };
                let item = item_box(&at.mark.pieces, at.top, g.rows, &g.metrics, g.scale)?;
                Some((
                    ThumbJob {
                        seq,
                        base: t.link_base(text).to_string(),
                        text: text.clone(),
                        alts: alts.clone(),
                    },
                    item,
                ))
            });
            match job {
                Some((job, item)) => {
                    st.item = Some(item);
                    st.worker.submit(job);
                }
                None => {
                    st.hover.arrived(seq, false);
                }
            }
        }
        st.worker.want(st.hover.loading_seq());
        if came_down {
            (self.deps.set_thumb)(None);
        }
        if st.hover.active() {
            due.push(now + THUMB_RECHECK_MS);
        }
    }

    /// A job answered. The next one may start; this one goes up only if its link is still under the cell.
    fn thumb_arrived(&mut self, tab: Option<&TabInfo>, seq: u64, thumb: Option<Arc<Thumb>>) {
        let Some(st) = self.thumbs.as_mut() else {
            return;
        };
        st.worker.finished(seq);
        let here = tab.and_then(|t| thumb_at(t, st.cell));
        let came_down = st.hover.recheck(here.as_ref());
        let up = st.hover.arrived(seq, thumb.is_some());
        st.worker.want(st.hover.loading_seq());
        if came_down {
            (self.deps.set_thumb)(None);
        }
        if up && let (Some(thumb), Some(item)) = (thumb, st.item) {
            (self.deps.set_thumb)(Some(&ThumbView { thumb, item }));
        }
    }

    /// The menu's Open and Show in Explorer: a worker re-resolves the link and applies the allowlist. A file link's
    /// spellings are tried in order; when none names anything no opener is called and the panel says so, as the
    /// click does (rule 273).
    fn open_path_async(&self, tab: String, base: String, texts: Vec<String>, mode: OpenMode) {
        let openers = Arc::clone(&self.deps.openers);
        let home = self.deps.home.clone();
        let post = Arc::clone(&self.deps.post);
        let started = Instant::now();
        let how = match mode {
            OpenMode::Open => "menu-open",
            OpenMode::Reveal => "menu-reveal",
        };
        thread::spawn(move || {
            let Some(text) = first_existing(&base, &texts, &home) else {
                let text = texts.into_iter().next().unwrap_or_default();
                link_crumb(how, "missing", &text, started);
                post(TermMsg::NoSuchFile { tab, text });
                return;
            };
            let got = open_term_path(&base, &text, mode, &*openers, &home);
            link_crumb(how, route_of(got), &text, started);
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
        self.sync_inputs(tab);
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
                self.hide_thumb();
                // A press starts a selection of the person's own: the prompt's is let go (rule 1235).
                self.prompt_sel = None;
                self.on_down(tab, &e, &hit, row_c, dev, mouse_owned, now);
            }
            1 => out.open_menu = self.on_up(tab, g, &e, &hit, row_c, dev, now),
            2 => self.on_move(tab, g, &e, &hit, row_c, dev.1, mouse_owned),
            _ => {
                self.machine.release();
                self.last_drag_scroll = None;
                self.held = None;
                self.reporting = false;
                self.pointer_left();
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
                // A press on a link is Flare's even when the program owns the mouse: the pointer already
                // shows the hand there, and a left click opens it (owner 2026-10-25: Claude Code's links did
                // not open, only the menu's "Open link" did). The program gets neither the press nor the release.
                let on_link = mouse_owned
                    && tab
                        .handle
                        .with_emu_quiet(|em| em.link_at_view(row_c, col))
                        .is_some();
                if mouse_owned && !on_link {
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
                    at: now,
                });
                self.redraw();
            }
            2 => {
                let has = tab.handle.with_emu_quiet(|em| em.has_selection());
                if mouse_owned && !has && (self.deps.right_click_reports)() {
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
        self.thumb_point(tab, g, (row_c, col), mark.clone());
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
                // Alt+click moves the cursor (xterm `altClickMovesCursor`, on in Prism Terminal):
                // a quick Alt press and release that selected at most one character sends the
                // arrows from the cursor to the cell under the release. A link under it still opens.
                if e.alt && now.saturating_sub(p.at) < ALT_CLICK_MS {
                    let c = col.min(g.cols.saturating_sub(1));
                    let keys = tab.handle.with_emu_quiet(|em| {
                        if em.sel_text().chars().count() <= 1 {
                            em.alt_click_plan(row_c, c)
                        } else {
                            String::new()
                        }
                    });
                    if !keys.is_empty() {
                        tab.handle.send_text(&keys);
                    }
                }
                let dragged = (dev.0 - p.x_dev).hypot(dev.1 - p.y_dev)
                    > f64::from(g.metrics.cell_w) / 2.0
                    || has;
                if dragged {
                    return None;
                }
                let release = tab.handle.with_emu_quiet(|em| em.link_at_view(row_c, col));
                match (&p.mark, &release) {
                    (Some(a), Some(b)) if a == b => self.click_link(tab, a),
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
                let ctx = tab.handle.with_emu_quiet(|em| em.context_at(row_c, col));
                let path = ctx
                    .link
                    .as_ref()
                    .and_then(|m| marked_path_at(&m.target, tab, &self.deps.home));
                let store = MenuStore {
                    tab: tab.id.to_string(),
                    x: e.x_css as f32,
                    y: e.y_css as f32,
                    selection: ctx.selection,
                    link: ctx.link_text,
                    path,
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

    /// A plain left click on a link mark. A web address opens through `open_url`; a file link is
    /// handed to a worker that re-resolves it and applies the allowlist. A relative path is read
    /// from the tab's folder (the scan only marks one when the folder is known).
    fn click_link(&mut self, tab: &TabInfo, mark: &LinkMark) {
        let shown = tab.handle.with_emu_quiet(|em| em.shown_for(mark));
        let base = match &mark.target {
            LinkTarget::Path { text, .. } => tab.link_base(text),
            _ => tab.cwd.unwrap_or_default(),
        };
        match click_action(&mark.target, shown.as_deref(), Some(base), 0) {
            LinkAction::OpenUrl(u) => self.open_link(&u),
            // A remote tab's file link names a file of the host: never resolved or opened here; the app has the tab's
            // link fetch it (rule 303).
            LinkAction::OpenPath { text, .. } if tab.remote => {
                let LinkTarget::Path { line, alts, .. } = &mark.target else {
                    return;
                };
                (self.deps.post)(TermMsg::RemoteLink {
                    tab: tab.id.to_string(),
                    texts: std::iter::once(text).chain(alts.iter().cloned()).collect(),
                    line: *line,
                });
            }
            LinkAction::OpenPath { base, text } => {
                let LinkTarget::Path { line, alts, .. } = &mark.target else {
                    return;
                };
                let texts = std::iter::once(text).chain(alts.iter().cloned()).collect();
                self.click_path_async(tab.id.to_string(), base, texts, *line);
            }
            LinkAction::Nothing => {}
        }
    }

    // ---- keys ----

    /// Called for a key the chord table did not take, with `term_key_action`'s answer.
    pub fn on_key(&mut self, tab: &TabInfo, ev: &KeyEvent, action: TermKeyAction) -> KeyDone {
        match action {
            TermKeyAction::Copy => {
                self.copy_selection(tab);
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
            TermKeyAction::SelectAll => {
                // macOS Cmd+A: the whole buffer, history and screen.
                tab.handle.with_emu(|em| em.sel_all());
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
                // Ctrl+Backspace alone, for an agent that deletes a word on another key (agy: ^W).
                if action == TermKeyAction::Pty
                    && ev.pressed
                    && ev.key == "Backspace"
                    && ev.ctrl
                    && !ev.shift
                    && !ev.alt
                    && !ev.meta
                    && let Some(k) = word_rubout_key(self.deps.agent.armed(tab.id))
                {
                    tab.handle.send_text(k);
                    return KeyDone::Handled;
                }
                KeyDone::Send
            }
        }
    }

    /// A key for an agent's prompt (rule 1235), called only while agent hotkeys are on and Claude runs in the tab:
    /// a prompt key acts, and a key over Flare's prompt selection replaces, deletes or cuts it. `None`: the key
    /// goes on its usual way (after any bytes sent here, such as the deletion of the selection it replaces).
    pub fn prompt_key(&mut self, tab: &TabInfo, ev: &KeyEvent, os: KeyOs) -> Option<KeyDone> {
        let input = pk::classify(ev, os);
        match input {
            Input::Ignore => return None,
            Input::Other => {
                self.prompt_sel = None;
                return None;
            }
            Input::Key(k @ (PromptKey::PrevMessage | PromptKey::NextMessage)) => {
                return self.message_key(tab, k == PromptKey::PrevMessage);
            }
            _ => {}
        }
        let (text, bounds, app_cursor) = tab.handle.with_emu_quiet(|em| {
            (
                pk::read_box(&em.prompt_lines(), em.cols()),
                em.sel_bounds(),
                em.mode().app_cursor,
            )
        });
        let Some(text) = text else {
            self.prompt_sel = None;
            return None;
        };
        // Flare's own selection, made on this tab over this very text; else one the mouse made in the box.
        let now_text = text.text();
        let sel = self
            .prompt_sel
            .take()
            .filter(|(t, s)| t == tab.id && s.text == now_text)
            .map(|(_, s)| s)
            .or_else(|| {
                let (s, e, block) = bounds?;
                let (a, b) = (!block).then(|| text.range_of_cells(s, e))??;
                Some(PromptSel {
                    anchor: a,
                    head: b,
                    caret: text.caret,
                    text: now_text.clone(),
                })
            });
        let plan = pk::plan(input, &text, sel.as_ref(), app_cursor, os);
        if !plan.send.is_empty() {
            tab.handle.send_text(&plan.send);
        }
        match plan.highlight {
            Highlight::Keep => {}
            Highlight::Clear => tab.handle.with_emu(|em| em.sel_clear()),
            Highlight::Set(a, b) => {
                if let Some((s, e)) = text.cells_of(a, b) {
                    tab.handle.with_emu(|em| em.sel_cells(s, e));
                }
            }
        }
        if let Some(cut) = plan.copy {
            self.copy(cut, COPY_MAX);
        }
        self.prompt_sel = plan.sel.map(|s| (tab.id.to_string(), s));
        self.redraw();
        (!plan.pass).then_some(KeyDone::Handled)
    }

    /// Ctrl+Shift+Up and Down: the view to your previous or next message (`❯` in the first column, the box's own
    /// row left out). A full-screen agent has no scrollback here: the key is the agent's then.
    fn message_key(&mut self, tab: &TabInfo, up: bool) -> Option<KeyDone> {
        if tab.handle.mode().alt_screen {
            return None;
        }
        let step = tab.handle.with_emu_quiet(|em| {
            let box_top =
                pk::read_box(&em.prompt_lines(), em.cols()).map_or(usize::MAX, |t| t.rows.0);
            let marks: Vec<usize> = em
                .marked_rows('\u{276f}')
                .into_iter()
                .filter(|&r| r < box_top)
                .collect();
            pk::message_scroll(&marks, em.viewport_top(), em.history_size(), up)
        });
        if let Some(d) = step {
            tab.handle.scroll_view(d);
            self.redraw();
        }
        Some(KeyDone::Handled)
    }

    /// The prompt's selection stands while its text does: a highlight a repaint dropped while the text is the same
    /// is put back; another text lets the selection go. True when the highlight was put back.
    fn prompt_tick(&mut self, tab: Option<&TabInfo>) -> bool {
        let Some((id, s)) = self.prompt_sel.clone() else {
            return false;
        };
        let Some(tab) = tab.filter(|t| t.id == id) else {
            self.prompt_sel = None;
            return false;
        };
        let (text, has) = tab.handle.with_emu_quiet(|em| {
            (
                pk::read_box(&em.prompt_lines(), em.cols()),
                em.has_selection(),
            )
        });
        let Some(text) = text.filter(|t| t.text() == s.text) else {
            self.prompt_sel = None;
            return false;
        };
        if has {
            return false;
        }
        let Some((a, b)) = s.range() else {
            return false;
        };
        let Some((start, end)) = text.cells_of(a, b) else {
            return false;
        };
        tab.handle.with_emu(|em| em.sel_cells(start, end));
        true
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

    /// A program in the front tab owns a selection it made itself (the agent guard leaves Ctrl+C
    /// to it).
    pub fn program_selecting(&self) -> bool {
        self.watch.selecting()
    }

    /// Whether a big copy begun on this tab is still waiting for its answer.
    pub fn copy_is_pending(&self, tab: &str) -> bool {
        self.copy_pending
            .lock()
            .unwrap()
            .get(tab)
            .is_some_and(|(_, since)| since.elapsed().as_millis() < u128::from(COPY_PENDING_MAX_MS))
    }

    pub fn has_selection(&self, tab: &TabInfo) -> bool {
        self.copy_is_pending(tab.id) || tab.handle.with_emu_quiet(|em| em.has_selection())
    }

    // ---- find ----

    /// Opens the bar for this tab, or closes it when it is already open here (D17).
    pub fn toggle_find(&mut self, tab: &TabInfo) {
        self.hide_thumb();
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

    /// Typing starts a search from the active match; the tick walks it a slice at a time and the
    /// match lands when it is found (the bar keeps its text until then).
    pub fn find_edited(&mut self, tab: &TabInfo, q: &str) {
        if !self.find.open || self.find.tab.as_deref() != Some(tab.id) {
            return;
        }
        self.find.query = q.to_string();
        tab.handle.with_emu(|em| {
            em.find_set(q);
            if !q.is_empty() {
                em.find_start(q, find::Dir::Next, true);
            }
        });
    }

    pub fn find_next(&mut self, tab: &TabInfo) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let q = self.find.query.clone();
        tab.handle
            .with_emu(|em| em.find_start(&q, find::Dir::Next, false));
    }

    pub fn find_prev(&mut self, tab: &TabInfo) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let q = self.find.query.clone();
        tab.handle
            .with_emu(|em| em.find_start(&q, find::Dir::Prev, false));
    }

    /// Closing clears the highlights and the selection the active match made (a selection the owner
    /// made stays), so the next Ctrl+C interrupts the program instead of copying the match.
    pub fn find_close(&mut self) {
        if let Some(h) = self.find.handle.take() {
            h.with_emu(|em| em.find_close());
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
        let searching = view.as_ref().is_some_and(|v| v.searching);
        let (count, index, done) = view.map_or((0, None, true), |v| (v.count, v.index, v.done));
        // While a new query is still being counted or searched and nothing is found yet, the
        // readout stays empty rather than saying "none" (Prism Terminal's search answers in one
        // pass). A search that lands moves the active match, and only then does the readout change.
        let counting = count == 0 && (!done || searching);
        FindBarState {
            open: self.find.open,
            query: self.find.query.clone(),
            readout: if counting {
                String::new()
            } else {
                find::readout(self.find.query.is_empty(), count, index)
            },
            can_step: count > 0,
        }
    }

    // ---- events and messages ----

    /// An OSC 52 write. Only the tab in front (`front`) may replace the clipboard: a background
    /// tab's write is dropped (audit 2026-10-20, L4, pastejacking). The writes of a flood share one
    /// worker and the last wins. Nothing is ever written back to the program.
    pub fn on_term_event(&mut self, tab: &str, front: Option<&str>, ev: &TermEvent) {
        if let TermEvent::ClipboardWrite(t) = ev {
            if front != Some(tab) {
                return;
            }
            let done = self.post_copied();
            self.osc_copier.submit(t.clone(), OSC52_MAX, done);
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
                // A remote tab's shell reads neither this machine's files nor its clipboard: copied files and an
                // image go to the host first, then their host paths are pasted (rule 304). Text pastes as text, and
                // the choice is a local tab's: Plain is text only, and an image beside text at a plain prompt is
                // the text (rule 1190).
                let agent = self.deps.agent.armed(t.id);
                if t.remote && remote_upload(&state, req.mode, agent) {
                    (self.deps.post)(TermMsg::RemoteUpload {
                        tab: t.id.to_string(),
                        files: state.files,
                        image: state.image,
                    });
                    return;
                }
                let shell = t.handle.shell_id().map(|s| path_shell_of(Some(s.as_str())));
                match paste_action(&state, req.mode, shell, agent) {
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
            TermMsg::BigCopied(tab_id, gen_no, ok) => {
                {
                    let mut p = self.copy_pending.lock().unwrap();
                    if p.get(&tab_id).is_some_and(|(g, _)| *g == gen_no) {
                        p.remove(&tab_id);
                    }
                }
                if ok == Some(true) {
                    self.badge.borrow_mut().copied(now);
                    self.redraw();
                }
            }
            // The app shows it (AppCtl::on_msg takes this before it gets here).
            TermMsg::Preview { .. }
            | TermMsg::PreviewRefused { .. }
            | TermMsg::NoSuchFile { .. }
            | TermMsg::RemoteLink { .. }
            | TermMsg::RemoteUpload { .. }
            | TermMsg::RemoteFileRefused { .. } => {}
            TermMsg::Thumb { seq, thumb } => self.thumb_arrived(tab, seq, thumb),
        }
    }

    /// A file dropped on the terminal area. Buffered only while the front tab is live. On a remote tab the batch is
    /// copied to the host first (rule 304): a path of this machine means nothing to its shell.
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

    /// Tests: a menu open on `tab` over `selection`, without a right click.
    #[cfg(test)]
    pub fn test_set_menu(&mut self, tab: &str, selection: String) {
        self.menu = Some(MenuStore {
            tab: tab.to_string(),
            x: 0.0,
            y: 0.0,
            selection,
            link: None,
            path: None,
        });
    }

    /// The grid selection the open menu was made over, if it was made over one.
    pub fn menu_selection(&self) -> Option<&str> {
        self.menu
            .as_ref()
            .map(|m| m.selection.as_str())
            .filter(|s| !s.is_empty())
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
                let texts = path.spellings();
                self.open_path_async(t.id.to_string(), path.base, texts, mode);
            }
            TermMenuAct::ShowInPanel { path } => {
                let texts = path.spellings();
                self.preview_path_async(t.id.to_string(), path.base, texts, path.line);
            }
            TermMenuAct::CopyPath { path } => {
                let texts = path.spellings();
                self.copy_path_async(path.base, texts);
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
    pub fn tab_changed(&mut self, _front: Option<&str>) {
        self.menu = None;
        self.last_rows.borrow_mut().clear();
        self.find_close();
        self.machine.release();
        self.last_drag_scroll = None;
        self.held = None;
        self.reporting = false;
        self.press = None;
        self.set_hover(None);
        self.pointer_left();
    }

    pub fn tab_closed(&mut self, id: &str) {
        self.copy_pending.lock().unwrap().remove(id);
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
    pub fn tick(&mut self, tab: Option<&TabInfo>, g: &GridGeom) -> TickOut {
        let now = self.now();
        let mut out = TickOut::default();
        let mut due: Vec<u64> = Vec::new();
        if let Some(t) = tab {
            self.sync_inputs(t);
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
            if h.with_emu_quiet(|em| em.find_busy()) {
                let t0 = Instant::now();
                let clock = move || t0.elapsed().as_secs_f64() * 1000.0;
                if let Some(scroll) = h.with_emu(|em| em.find_step(&clock)) {
                    h.scroll_view(-scroll);
                }
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

        self.thumb_tick(tab, g, now, &mut due);
        if self.prompt_tick(tab) {
            out.redraw = true;
        }
        if self.prompt_sel.is_some() {
            due.push(now + PROMPT_SEL_RECHECK_MS);
        }

        if let Some(paths) = self.drops.take_ready(now) {
            if let Some(t) = tab.filter(|t| t.alive) {
                if t.remote {
                    // Never this machine's paths typed into the host's shell: the files go over first (rule 304).
                    (self.deps.post)(TermMsg::RemoteUpload {
                        tab: t.id.to_string(),
                        files: paths,
                        image: false,
                    });
                } else {
                    type_dropped(t.handle, &paths);
                }
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
    #[test]
    fn the_worn_theme_link_colour_reaches_the_ink() {
        set_worn_link("#1a4f9c");
        let ink = worn_ink().expect("worn");
        assert_eq!(ink.link, flare_render::color::Rgba::rgb(0x1a, 0x4f, 0x9c));
        assert_eq!(ink.selection, DecorInk::pt_default().selection);
    }

    /// Review #52: the find fills come from the worn theme, not from PT's fixed dark grey.
    #[test]
    fn the_worn_theme_find_colours_reach_the_ink() {
        set_worn_link("#1a4f9c");
        let find = flare_theme::find_ink::find_ink("#fafafa", "#383a42", "#4078f2");
        set_worn_find(&find);
        let ink = worn_ink().expect("worn");
        let hex = |c: flare_render::color::Rgba| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
        assert_eq!(hex(ink.find_bg), find.bg);
        assert_eq!(hex(ink.active_bg), find.active_bg);
        assert_ne!(ink.find_bg, DecorInk::pt_default().find_bg);
    }
    use super::*;
    use crate::openers::{OpenedRecord, RecordingOpeners};
    use flare_agent::kinds::AgentKind;
    use flare_render::glyph::{FakeGlyphs, GlyphSource};
    use flare_render::hit::SCALES;
    use flare_render::metrics::cell_metrics;
    use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
    use flare_term::handle::StartOptions;
    use flare_term::keys::{KeyState, term_key_action};
    use flare_term::sel_machine::{Anchor, SelSpec};
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

    /// Every `set_thumb` call in order.
    type ThumbCalls = Rc<RefCell<Vec<Option<(String, BoxF)>>>>;

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
        /// Every `set_thumb` call: the caption and box of a picture going up, None for one coming down.
        thumbs: ThumbCalls,
        agent: Rc<TestAgent>,
        rcr: Rc<Cell<bool>>,
        g: GridGeom,
        cwd: Option<String>,
        /// The tab is a remote one (remote sessions PR 6).
        remote: bool,
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
            let thumbs = Rc::new(RefCell::new(Vec::new()));
            let agent = Rc::new(TestAgent(Cell::new(None)));
            let rcr = Rc::new(Cell::new(false));
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
                set_thumb: {
                    let t = Rc::clone(&thumbs);
                    Rc::new(move |v: Option<&ThumbView>| {
                        t.borrow_mut().push(v.map(|v| (v.thumb.caption(), v.item)));
                    })
                },
                agent: agent.clone(),
                home: std::env::temp_dir().to_string_lossy().to_string(),
                dbl: FALLBACK,
                right_click_reports: {
                    let f = Rc::clone(&rcr);
                    Rc::new(move || f.get())
                },
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
                thumbs,
                agent,
                rcr,
                g: GridGeom {
                    metrics,
                    scale,
                    cols: COLS,
                    rows: ROWS,
                    window: (800, 600),
                },
                cwd,
                remote: false,
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
                shell_cwd: None,
                alive: true,
                remote: self.remote,
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
                shell_cwd: None,
                alive: true,
                remote: self.remote,
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
                shell_cwd: None,
                alive: true,
                remote: self.remote,
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
                shell_cwd: None,
                alive: true,
                remote: self.remote,
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
                shell_cwd: None,
                alive: true,
                remote: self.remote,
            };
            self.ui.find_edited(&tab, q);
        }

        fn with_tab<R>(&mut self, f: impl FnOnce(&mut TermUi, &TabInfo) -> R) -> R {
            let h = self.handle.clone();
            let tab = TabInfo {
                id: "a",
                handle: &h,
                cwd: self.cwd.as_deref(),
                shell_cwd: None,
                alive: true,
                remote: self.remote,
            };
            f(&mut self.ui, &tab)
        }

        /// Ticks until the find count and any search have finished.
        fn settle_find(&mut self) {
            for _ in 0..3000 {
                if self.handle.with_emu_quiet(|e| !e.find_busy()) {
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

    /// Selects every row of history and screen, from the first cell to the last.
    fn select_everything(r: &Rig) {
        r.handle.with_emu(|e| {
            let total = e.history_size() + e.rows();
            e.sel_apply(SelOut::Set(SelSpec {
                block: false,
                start: Anchor { row: 0, b: 0 },
                end: Anchor {
                    row: total - 1,
                    b: COLS,
                },
            }));
        });
    }

    #[test]
    fn a_big_copy_is_read_on_a_worker_and_the_badge_waits_for_the_clipboard() {
        let mut r = Rig::new(1.0);
        r.feed(&(0..300).map(|i| format!("row {i}\r\n")).collect::<String>());
        select_everything(&r);
        let want = r.text();
        r.key(ctrl("c", "KeyC"));
        // Nothing shows until the UI has the answer from the worker.
        assert_eq!(r.badge(), BadgePhase::Gone);
        until("the copy", || {
            r.pump();
            r.clip.read_text().is_some()
        });
        assert_eq!(r.clip.read_text(), Some(want));
        assert!(!r.has_sel());
        assert_eq!(r.badge(), BadgePhase::Shown);
    }

    #[test]
    fn a_big_copy_that_the_clipboard_refuses_raises_no_badge() {
        let mut r = Rig::with_port(1.0, Arc::new(Refusing));
        r.feed(&(0..300).map(|i| format!("row {i}\r\n")).collect::<String>());
        select_everything(&r);
        r.key(ctrl("c", "KeyC"));
        until("the answer", || {
            r.pump();
            !r.ui.copy_is_pending("a")
        });
        assert_eq!(r.badge(), BadgePhase::Gone);
    }

    #[test]
    fn ctrl_c_with_a_selection_in_flight_is_not_the_interrupt() {
        let mut r = Rig::new(1.0);
        r.feed(&(0..300).map(|i| format!("row {i}\r\n")).collect::<String>());
        select_everything(&r);
        r.key(ctrl("c", "KeyC"));
        let (a, d) = r.key(ctrl("c", "KeyC"));
        assert_eq!((a, d), (TermKeyAction::Copy, KeyDone::Handled));
    }

    #[test]
    fn a_second_copy_supersedes_an_unfinished_first() {
        let mut r = Rig::new(1.0);
        r.feed(&(0..300).map(|i| format!("row {i}\r\n")).collect::<String>());
        select_everything(&r);
        let want = r.text();
        r.key(ctrl("c", "KeyC"));
        r.key(ctrl("c", "KeyC"));
        until("the copy", || {
            r.pump();
            r.clip.read_text().is_some()
        });
        assert_eq!(r.clip.read_text(), Some(want));
    }

    #[test]
    #[ignore = "release timing"]
    fn pressing_ctrl_c_on_10000_lines_costs_the_ui_thread_under_2_ms() {
        let mut r = Rig::new(1.0);
        r.feed(
            &(0..10_000)
                .map(|i| format!("line {i}\r\n"))
                .collect::<String>(),
        );
        select_everything(&r);
        let started = Instant::now();
        r.key(ctrl("c", "KeyC"));
        let cost = started.elapsed();
        assert!(cost < Duration::from_millis(2), "the key cost {cost:?}");
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

    /// The words of `text` that name a file (they hold a dot) between the marks a session writes (`file_marks`).
    fn marked(text: &str) -> String {
        let line = |l: &str| {
            l.split(' ')
                .map(|w| {
                    if w.contains('.') {
                        format!("\u{27E6}{w}\u{27E7}")
                    } else {
                        w.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        text.split("\r\n")
            .map(line)
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    /// Prints the texts with their file names between marks: they are links at once, with no stat.
    fn light_up(r: &mut Rig, text: &str) {
        r.feed(&marked(text));
        r.tick(0);
    }

    #[test]
    fn a_marked_path_lights_up_at_once_and_a_plain_one_never() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.feed("a.txt\r\n\u{27E6}b.txt\u{27E7}");
        r.tick(0);
        let d = r.frame();
        assert!(
            !d.links.iter().any(|s| s.row == 0),
            "a path with no marks is never guessed, even one that exists"
        );
        assert!(
            d.links
                .iter()
                .any(|s| s.row == 1 && s.col == 0 && s.width == 7),
            "the session marked it on purpose: no stat before it lights"
        );
    }

    /// Rule 303: on a remote tab a click on a file link names a file of the host: it is never resolved or opened here,
    /// even when this machine has a file of that name; its spellings go to the app for the tab's link to fetch.
    #[test]
    fn a_click_on_a_remote_tabs_file_link_asks_the_link_and_opens_nothing_here() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.remote = true;
        light_up(&mut r, "a.txt");
        r.frame();
        r.click(2, 0);
        let got = r.msgs.lock().unwrap().iter().find_map(|m| match m {
            TermMsg::RemoteLink { tab, texts, .. } => Some((tab.clone(), texts.clone())),
            _ => None,
        });
        assert_eq!(got, Some(("a".to_string(), vec!["a.txt".to_string()])));
        thread::sleep(Duration::from_millis(50));
        assert!(r.openers.records().is_empty(), "nothing opened here");
        assert!(
            !r.msgs
                .lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, TermMsg::Preview { .. } | TermMsg::NoSuchFile { .. })),
            "this machine's file is never previewed"
        );
    }

    #[test]
    fn a_click_on_a_marked_path_that_names_nothing_says_so_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "gone.txt");
        r.frame();
        r.click(2, 0);
        until("the word", || {
            r.msgs
                .lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, TermMsg::NoSuchFile { tab, text } if tab == "a" && text == "gone.txt"))
        });
        assert!(previews(&r).is_empty());
        assert!(r.openers.records().is_empty(), "{:?}", r.openers.records());
    }

    #[test]
    fn a_click_on_a_marked_line_asks_for_the_preview_at_that_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "1\n2\n3\n").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.feed("\u{27E6}a.txt:3\u{27E7}");
        r.tick(0);
        r.frame();
        r.click(2, 0);
        until("the preview", || !previews(&r).is_empty());
        let lines: Vec<Option<u32>> = r
            .msgs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| match m {
                TermMsg::Preview { line, .. } => Some(*line),
                _ => None,
            })
            .collect();
        assert_eq!(lines, [Some(3)]);
    }

    /// A path with a space where Claude Code broke the row: nothing between the rows names nothing, one space does.
    #[test]
    fn a_click_tries_a_space_where_the_row_broke_when_the_joined_path_names_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("My Docs")).unwrap();
        std::fs::write(dir.path().join("My Docs").join("a.pdf"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        let cols = r.g.cols;
        let head = format!("{}\u{27E6}My", " ".repeat(cols - 3));
        r.feed(&format!("{head}\r\n  Docs/a.pdf\u{27E7}"));
        r.tick(0);
        r.frame();
        r.click(4, 1);
        until("the open", || !r.openers.records().is_empty());
        let rec = r.openers.records();
        assert!(
            matches!(&rec[0], OpenedRecord::Open(p) if p.ends_with("a.pdf") && p.contains("My Docs")),
            "{rec:?}"
        );
    }

    /// The same link as above, through the menu: every row acts on the spelling that names something.
    #[test]
    fn the_menu_rows_act_on_the_spelling_with_a_space_where_the_row_broke() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("My Docs")).unwrap();
        std::fs::write(dir.path().join("My Docs").join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        let cols = r.g.cols;
        let head = format!("{}\u{27E6}My", " ".repeat(cols - 3));
        r.feed(&format!("{head}\r\n  Docs/a.txt\u{27E7}"));
        r.tick(0);
        r.frame();
        let pick = |r: &mut Rig, label: &str| {
            r.right_click(4, 1).open_menu.expect("a menu");
            let rows = r.ui.menu_rows(false, "Ctrl+F", "");
            let i = rows.iter().position(|x| x.label == label).expect(label);
            r.with_tab(|ui, t| ui.on_menu_pick(Some(t), i));
        };
        pick(&mut r, "Copy path");
        until("the copy", || r.clip.read_text().is_some());
        let copied = r.clip.read_text().unwrap();
        assert!(copied.contains("My Docs"), "{copied}");
        pick(&mut r, "Show in side panel");
        until("the preview", || !previews(&r).is_empty());
        let got = previews(&r);
        assert!(
            got[0].1 == "text" && got[0].2.contains("My Docs"),
            "{got:?}"
        );
        pick(&mut r, flare_win::openers::REVEAL_LABEL);
        until("the reveal", || !r.openers.records().is_empty());
        let rec = r.openers.records();
        assert!(
            matches!(&rec[0], OpenedRecord::Reveal(p) if p.contains("My Docs")),
            "{rec:?}"
        );
    }

    #[test]
    fn a_path_link_click_opens_through_the_allowlist_on_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.pdf"), "x").unwrap();
        std::fs::write(dir.path().join("run.bat"), "x").unwrap();
        std::fs::create_dir(dir.path().join("x.d")).unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.pdf run.bat x.d");
        r.frame();
        r.click(2, 0);
        r.click(10, 0);
        r.click(20, 0);
        until("three opener calls", || r.openers.records().len() >= 3);
        // Three workers run at once, so the order is theirs.
        let rec = r.openers.records();
        let has = |f: &dyn Fn(&OpenedRecord) -> bool| rec.iter().any(f);
        assert!(
            has(&|x| matches!(x, OpenedRecord::Open(p) if p.ends_with("a.pdf"))),
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

    /// The Preview messages posted so far: (tab, kind name, path).
    fn previews(r: &Rig) -> Vec<(String, String, String)> {
        r.msgs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| match m {
                TermMsg::Preview { tab, kind, abs, .. } => {
                    Some((tab.clone(), kind.name(), abs.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_click_on_a_text_file_or_a_picture_asks_for_a_preview_and_a_script_is_still_revealed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join("p.png"), "x").unwrap();
        std::fs::write(dir.path().join("run.bat"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.txt p.png run.bat");
        r.frame();
        r.click(2, 0);
        r.click(10, 0);
        r.click(18, 0);
        until("two previews and a reveal", || {
            previews(&r).len() >= 2 && !r.openers.records().is_empty()
        });
        let got = previews(&r);
        assert!(
            got.iter()
                .any(|(_, k, p)| k == "text" && p.ends_with("a.txt")),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|(_, k, p)| k == "image" && p.ends_with("p.png")),
            "{got:?}"
        );
        let rec = r.openers.records();
        assert_eq!(rec.len(), 1, "{rec:?}");
        assert!(
            matches!(&rec[0], OpenedRecord::Reveal(p) if p.ends_with("run.bat")),
            "{rec:?}"
        );
    }

    #[test]
    fn show_in_side_panel_asks_for_the_preview_the_click_asks_for_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.ui.menu = Some(MenuStore {
            tab: "a".into(),
            x: 0.0,
            y: 0.0,
            selection: String::new(),
            link: None,
            path: Some(TermPathAt {
                kind: flare_term::path_open::PathKind::File,
                abs: format!("{base}/a.txt"),
                text: "a.txt".into(),
                base,
                line: None,
                alts: Vec::new(),
            }),
        });
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let i = rows
            .iter()
            .position(|x| x.label == "Show in side panel")
            .expect("the row");
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), i));
        until("the preview", || !previews(&r).is_empty());
        let got = previews(&r);
        assert!(got[0].1 == "text" && got[0].2.ends_with("a.txt"), "{got:?}");
        assert!(r.openers.records().is_empty(), "{:?}", r.openers.records());
    }

    #[test]
    fn show_in_side_panel_on_a_file_that_went_away_says_flare_found_no_such_file_and_opens_nothing()
    {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.ui.menu = Some(MenuStore {
            tab: "a".into(),
            x: 0.0,
            y: 0.0,
            selection: String::new(),
            link: None,
            path: Some(TermPathAt {
                kind: flare_term::path_open::PathKind::File,
                abs: format!("{base}/a.txt"),
                text: "a.txt".into(),
                base,
                line: None,
                alts: Vec::new(),
            }),
        });
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let i = rows
            .iter()
            .position(|x| x.label == "Show in side panel")
            .expect("the row");
        // The menu was built with the file there; it is gone when the row is picked.
        std::fs::remove_file(&file).unwrap();
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), i));
        until("the word", || no_such_file(&r, "a.txt"));
        assert!(previews(&r).is_empty());
        assert!(r.openers.records().is_empty(), "{:?}", r.openers.records());
    }

    #[test]
    fn show_in_side_panel_on_a_file_that_is_there_but_not_shown_says_it_cannot_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A folder with a text file's name: it names something, so it is no "no such file", and it is not shown.
        std::fs::create_dir(dir.path().join("a.txt")).unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.txt");
        r.frame();
        pick_on(&mut r, 2, 0, "Show in side panel");
        until("the refusal", || {
            r.msgs
                .lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, TermMsg::PreviewRefused { tab } if tab == "a"))
        });
        assert!(!no_such_file(&r, "a.txt"));
        assert!(r.openers.records().is_empty(), "{:?}", r.openers.records());
    }

    /// Whether the panel was asked to say that `text` names nothing.
    fn no_such_file(r: &Rig, text: &str) -> bool {
        r.msgs
            .lock()
            .unwrap()
            .iter()
            .any(|m| matches!(m, TermMsg::NoSuchFile { tab, text: t } if tab == "a" && t == text))
    }

    /// Opens the menu on the link at (`col`, `row`) and picks the row with this label.
    fn pick_on(r: &mut Rig, col: usize, row: usize, label: &str) {
        r.right_click(col, row).open_menu.expect("a menu");
        let rows = r.ui.menu_rows(false, "Ctrl+F", "");
        let i = rows.iter().position(|x| x.label == label).expect(label);
        r.with_tab(|ui, t| ui.on_menu_pick(Some(t), i));
    }

    /// Owner, 2026-11-11: a table link whose spellings name nothing must never reach an opener, on any route. Each row
    /// says "Flare found no such file" as the click does, with the text as written.
    #[test]
    fn a_file_link_that_names_nothing_reaches_no_opener_from_the_click_or_any_menu_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        let cols = r.g.cols;
        // Broken by hand, so it has a second spelling; neither names anything.
        let head = format!("{}\u{27E6}My", " ".repeat(cols - 3));
        r.feed(&format!("{head}\r\n  Docs/gone.txt\u{27E7}"));
        r.tick(0);
        r.frame();
        let says = |r: &Rig| {
            r.msgs
                .lock()
                .unwrap()
                .iter()
                .filter(|m| matches!(m, TermMsg::NoSuchFile { tab, text } if tab == "a" && text == "MyDocs/gone.txt"))
                .count()
        };
        r.click(4, 1);
        until("the click's word", || says(&r) == 1);
        for (n, label) in [
            "Open",
            flare_win::openers::REVEAL_LABEL,
            "Show in side panel",
        ]
        .into_iter()
        .enumerate()
        {
            pick_on(&mut r, 4, 1, label);
            until(label, || says(&r) == n + 2);
        }
        assert!(previews(&r).is_empty());
        assert!(r.openers.records().is_empty(), "{:?}", r.openers.records());
    }

    /// Review (2026-11-11): the agent's folder reads relative links only; a rooted path with no drive and `~` are read
    /// from the tab's own folder, as before the agent's folder was known.
    #[test]
    fn only_a_relative_file_link_is_read_from_the_agents_folder() {
        let r = Rig::new(1.0);
        let tab = TabInfo {
            id: "a",
            handle: &r.handle,
            cwd: Some("AGENT"),
            shell_cwd: Some("SHELL"),
            alive: true,
            remote: false,
        };
        assert_eq!(tab.link_base("src/a.rs"), "AGENT");
        assert_eq!(tab.link_base("./a.rs"), "AGENT");
        assert_eq!(tab.link_base("/x/a.rs"), "SHELL");
        assert_eq!(tab.link_base("~/a.rs"), "SHELL");
        let alone = TabInfo {
            shell_cwd: None,
            ..tab
        };
        assert_eq!(alone.link_base("/x/a.rs"), "AGENT");
    }

    #[test]
    fn copy_path_copies_the_spelling_that_exists_as_a_whole_path_else_the_text_as_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        light_up(&mut r, "a.txt b.txt");
        r.frame();
        pick_on(&mut r, 2, 0, "Copy path");
        let a = dir.path().join("a.txt").to_string_lossy().into_owned();
        until("the whole path", || {
            r.clip.read_text().as_deref() == Some(a.as_str())
        });
        pick_on(&mut r, 10, 0, "Copy path");
        until("the text as written", || {
            r.clip.read_text().as_deref() == Some("b.txt")
        });
    }

    #[test]
    fn with_previews_off_no_preview_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = Rig::with_cwd(1.0, dir.path());
        r.ui.set_previews(false);
        light_up(&mut r, "a.txt");
        r.frame();
        r.click(2, 0);
        until("the open", || !r.openers.records().is_empty());
        assert!(previews(&r).is_empty());
    }

    // ---- hover thumbnails ----

    fn png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([9, 8, 7, 255]))
            .save(path)
            .unwrap();
    }

    /// A rig with thumbnails on and these words printed and lit up, in a folder holding the pictures named.
    fn thumb_rig(dir: &Path, text: &str) -> Rig {
        let mut r = Rig::with_cwd(1.0, dir);
        r.ui.set_thumbs(true);
        light_up(&mut r, text);
        r.frame();
        r
    }

    /// Hands every worker answer that has come in to the UI side.
    fn pump_thumbs(r: &mut Rig) {
        let h = r.handle.clone();
        let tab = TabInfo {
            id: "a",
            handle: &h,
            cwd: r.cwd.as_deref(),
            shell_cwd: None,
            alive: true,
            remote: false,
        };
        let all: Vec<TermMsg> = r.msgs.lock().unwrap().drain(..).collect();
        for m in all {
            r.ui.on_msg(Some(&tab), m);
        }
    }

    fn has_thumb_msg(r: &Rig) -> bool {
        r.msgs
            .lock()
            .unwrap()
            .iter()
            .any(|m| matches!(m, TermMsg::Thumb { .. }))
    }

    /// Rests on the cell, waits out the rest and hands the answer over.
    fn rest_on(r: &mut Rig, col: usize, row: usize) {
        r.hover(col, row);
        r.tick(0);
        r.tick(crate::thumbs::DWELL_MS);
        until("the thumbnail's answer", || has_thumb_msg(r));
        pump_thumbs(r);
    }

    fn last_thumb(r: &Rig) -> Option<Option<(String, BoxF)>> {
        r.thumbs.borrow().last().cloned()
    }

    fn is_up(r: &Rig) -> bool {
        matches!(last_thumb(r), Some(Some(_)))
    }

    #[test]
    fn resting_on_an_image_path_shows_its_thumbnail_after_the_rest_and_not_before() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 600, 300);
        let mut r = thumb_rig(dir.path(), "p.png");
        r.hover(2, 0);
        r.tick(0);
        r.tick(crate::thumbs::DWELL_MS - 1);
        assert!(!has_thumb_msg(&r), "no read before the rest is over");
        assert!(r.thumbs.borrow().is_empty());
        r.tick(1);
        until("the answer", || has_thumb_msg(&r));
        pump_thumbs(&mut r);
        let (caption, item) = last_thumb(&r).flatten().expect("the picture is up");
        assert_eq!(caption, "p.png  600 x 300");
        let m = &r.g.metrics;
        assert_eq!(item.top, f64::from(m.padding));
        assert_eq!(item.height, f64::from(m.cell_h));
    }

    #[test]
    fn a_path_that_is_not_a_picture_never_asks_for_a_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut r = thumb_rig(dir.path(), "a.txt");
        r.hover(2, 0);
        r.tick(0);
        r.tick(1000);
        thread::sleep(Duration::from_millis(30));
        assert!(!has_thumb_msg(&r));
        assert!(r.thumbs.borrow().is_empty());
    }

    #[test]
    fn with_the_setting_off_a_rest_starts_no_state_and_no_worker() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        r.ui.set_thumbs(false);
        assert!(r.ui.thumbs.is_none());
        r.hover(2, 0);
        r.tick(0);
        let out = r.tick(1000);
        assert!(r.ui.thumbs.is_none());
        assert!(out.next_due_ms.is_none_or(|d| d > r.clock.get() + 50));
        thread::sleep(Duration::from_millis(30));
        assert!(!has_thumb_msg(&r));
    }

    #[test]
    fn turning_the_setting_off_takes_the_picture_down() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        rest_on(&mut r, 2, 0);
        assert!(is_up(&r));
        r.ui.set_thumbs(false);
        assert_eq!(last_thumb(&r), Some(None));
    }

    #[test]
    fn scrolling_under_a_still_pointer_takes_the_picture_down_on_the_next_tick() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        rest_on(&mut r, 2, 0);
        assert!(is_up(&r));
        // Output pushes the line off the screen, and the pointer never moves.
        r.feed("\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
        r.frame();
        r.tick(50);
        assert_eq!(last_thumb(&r), Some(None));
    }

    /// The bug the per-tick look exists for: a still pointer, another picture's path scrolled under it.
    #[test]
    fn another_picture_scrolled_under_the_cell_is_not_shown_as_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("a.png"), 4, 4);
        png(&dir.path().join("b.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "a.png\r\nb.png\r\n\r\n\r\n\r\n");
        rest_on(&mut r, 2, 0);
        assert_eq!(last_thumb(&r).flatten().expect("up").0, "a.png  4 x 4");
        // One more line: b.png is now on the row the pointer rests on, and the pointer never moved.
        r.feed("\r\n");
        r.frame();
        r.tick(50);
        assert_eq!(last_thumb(&r), Some(None));
        r.tick(5000);
        assert_eq!(
            r.thumbs.borrow().iter().flatten().count(),
            1,
            "and b.png does not come up until the pointer moves"
        );
    }

    #[test]
    fn an_answer_for_a_link_that_scrolled_away_while_it_loaded_never_goes_up() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        r.hover(2, 0);
        r.tick(0);
        r.tick(crate::thumbs::DWELL_MS);
        until("the answer", || has_thumb_msg(&r));
        r.feed("\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
        r.frame();
        pump_thumbs(&mut r);
        assert!(r.thumbs.borrow().is_empty());
    }

    #[test]
    fn moving_off_the_link_takes_the_picture_down() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png ok");
        rest_on(&mut r, 2, 0);
        r.hover(7, 0);
        assert_eq!(last_thumb(&r), Some(None));
    }

    #[test]
    fn the_pointer_leaving_the_grid_takes_the_picture_down() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        rest_on(&mut r, 2, 0);
        r.ui.pointer_left();
        assert_eq!(last_thumb(&r), Some(None));
    }

    #[test]
    fn a_key_a_press_and_find_each_take_it_down_and_a_nudge_does_not_bring_it_back() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png ok");
        rest_on(&mut r, 2, 0);
        r.ui.hide_thumb();
        assert_eq!(last_thumb(&r), Some(None));
        r.hover(3, 0);
        r.tick(0);
        r.tick(2000);
        thread::sleep(Duration::from_millis(30));
        assert!(!has_thumb_msg(&r), "a nudge on the same link reads nothing");

        r.hover(7, 0);
        rest_on(&mut r, 2, 0);
        assert!(is_up(&r), "back after leaving and resting again");
        r.down(2, 0, false);
        assert_eq!(last_thumb(&r), Some(None), "a press");
        r.up(2, 0, false);
        pump_thumbs(&mut r);

        r.hover(7, 0);
        rest_on(&mut r, 2, 0);
        assert!(is_up(&r));
        let h = r.handle.clone();
        let tab = TabInfo {
            id: "a",
            handle: &h,
            cwd: r.cwd.as_deref(),
            shell_cwd: None,
            alive: true,
            remote: false,
        };
        r.ui.toggle_find(&tab);
        assert_eq!(last_thumb(&r), Some(None), "Find");
    }

    #[test]
    fn a_tab_switch_takes_it_down() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png");
        rest_on(&mut r, 2, 0);
        r.ui.tab_changed(None);
        assert_eq!(last_thumb(&r), Some(None));
    }

    #[test]
    fn a_resize_and_a_dpi_change_each_take_it_down() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png ok");
        rest_on(&mut r, 2, 0);
        r.g.window = (801, 600);
        r.tick(50);
        assert_eq!(last_thumb(&r), Some(None), "the window's size");

        r.hover(7, 0);
        rest_on(&mut r, 2, 0);
        assert!(is_up(&r));
        r.g.scale = 1.5;
        r.tick(50);
        assert_eq!(last_thumb(&r), Some(None), "the DPI");
    }

    #[test]
    fn a_tick_keeps_checking_while_a_picture_is_up_and_stops_when_it_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let mut r = thumb_rig(dir.path(), "p.png ok");
        rest_on(&mut r, 2, 0);
        let now = r.clock.get();
        assert!(r.tick(0).next_due_ms.is_some_and(|d| d <= now + 50));
        r.hover(7, 0);
        let now = r.clock.get();
        assert!(r.tick(0).next_due_ms.is_none_or(|d| d > now + 50));
    }

    #[test]
    fn a_folder_with_a_pictures_name_and_a_broken_picture_show_nothing_and_open_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("x.png")).unwrap();
        std::fs::write(dir.path().join("bad.png"), "not a picture").unwrap();
        let mut r = thumb_rig(dir.path(), "x.png bad.png");
        rest_on(&mut r, 2, 0);
        r.hover(9, 0);
        r.tick(0);
        r.tick(crate::thumbs::DWELL_MS);
        until("the answer", || has_thumb_msg(&r));
        pump_thumbs(&mut r);
        assert!(r.thumbs.borrow().is_empty());
        assert!(r.openers.records().is_empty());
        assert!(r.urls.borrow().is_empty());
    }

    #[test]
    fn a_web_address_of_a_picture_is_never_fetched_on_hover() {
        let mut r = Rig::new(1.0);
        r.ui.set_thumbs(true);
        r.feed("https://a.io/p.png");
        r.frame();
        r.hover(4, 0);
        r.tick(0);
        r.tick(1000);
        thread::sleep(Duration::from_millis(30));
        assert!(!has_thumb_msg(&r));
        assert!(r.thumbs.borrow().is_empty());
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
                line: None,
                alts: Vec::new(),
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
    fn a_click_on_a_link_opens_it_in_mouse_mode_and_the_program_gets_nothing() {
        let mut r = Rig::new(1.0);
        r.feed("see https://a.io/x ok");
        r.feed(MOUSE_ON);
        r.frame();
        r.click(8, 0);
        assert_eq!(*r.urls.borrow(), vec!["https://a.io/x".to_string()]);
        thread::sleep(Duration::from_millis(30));
        assert!(
            r.written().is_empty(),
            "no press or release reaches the program"
        );
        // Off the link the program still gets its clicks.
        r.down(1, 0, false);
        r.up(1, 0, false);
        let want = b"\x1b[<0;2;1M\x1b[<0;2;1m";
        assert_eq!(r.wait_written(want.len()), want);
        assert_eq!(r.urls.borrow().len(), 1);
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
    fn a_right_click_in_mouse_mode_opens_the_menu_and_sends_nothing() {
        let mut r = Rig::new(1.0);
        r.feed("hello world");
        r.feed(MOUSE_ON);
        let out = r.right_click(2, 0);
        assert!(out.open_menu.is_some());
        thread::sleep(Duration::from_millis(40));
        assert!(r.written().is_empty(), "nothing reaches the program");
        // The same with a selection standing.
        let a = r.css(0, 0, false);
        let b = r.css(5, 0, false);
        r.ptr_at(0, 0, a, true, false);
        r.ptr_at(2, 0, b, true, false);
        r.ptr_at(1, 0, b, true, false);
        let out = r.right_click(2, 0);
        assert!(out.open_menu.is_some());
        thread::sleep(Duration::from_millis(40));
        assert!(r.written().is_empty());
    }

    #[test]
    fn with_the_setting_on_a_right_press_is_reported_unless_a_selection_stands() {
        let mut r = Rig::new(1.0);
        r.rcr.set(true);
        r.feed("hello world");
        r.feed(MOUSE_ON);
        let out = r.right_click(2, 0);
        assert!(out.open_menu.is_some());
        let want = b"[<2;3;1M[<2;3;1m";
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
            shell_cwd: None,
            alive: true,
            remote: false,
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

    #[test]
    fn ctrl_backspace_where_agy_runs_sends_ctrl_w() {
        let mut r = Rig::new(1.0);
        let bs = || KeyEvent::new("Backspace", "Backspace").ctrl();
        assert_eq!(r.key(bs()), (TermKeyAction::Pty, KeyDone::Send));
        r.agent.0.set(Some(AgentKind::Claude));
        assert_eq!(r.key(bs()), (TermKeyAction::Pty, KeyDone::Send));
        r.agent.0.set(Some(AgentKind::Codex));
        assert_eq!(r.key(bs()), (TermKeyAction::Pty, KeyDone::Send));
        r.agent.0.set(Some(AgentKind::Agy));
        assert_eq!(r.key(bs().shift()), (TermKeyAction::Pty, KeyDone::Send));
        assert_eq!(r.key(bs().alt()), (TermKeyAction::Pty, KeyDone::Send));
        assert_eq!(r.key(bs().released()), (TermKeyAction::Pty, KeyDone::Send));
        assert_eq!(r.key(bs()), (TermKeyAction::Pty, KeyDone::Handled));
        assert_eq!(r.wait_written(1), b"\x17");
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
            r.settle_find();
            seen.push(r.ui.find_state().readout);
        }
        assert_eq!(seen, ["2 of 3", "3 of 3", "1 of 3"]);
        r.with_tab(|ui, t| ui.find_prev(t));
        r.settle_find();
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
        r.settle_find();
        assert_eq!(r.top(), before, "a match in view does not scroll");
        r.find_edited("needle");
        r.settle_find();
        assert!(r.top() < before, "a match outside the view scrolls");
        assert!(r.top() == 0);
    }

    #[test]
    fn the_readout_keeps_the_old_text_until_the_job_finishes_and_says_none_after() {
        let mut r = Rig::new(1.0);
        r.feed("foo x foo x foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        r.find_edited("foo");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1 of 3");
        // Enter starts a walk: the match has not landed, so the text stays.
        r.with_tab(|ui, t| ui.find_next(t));
        assert_eq!(r.ui.find_state().readout, "1 of 3");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "2 of 3");
        // A query with no match: never "none" while the walk runs, "none" once it has finished.
        r.find_edited("zzz");
        assert_ne!(r.ui.find_state().readout, "none");
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "none");
    }

    #[test]
    fn find_readout_and_buttons_follow_the_count() {
        let mut r = Rig::new(1.0);
        r.feed("foo");
        r.with_tab(|ui, t| ui.toggle_find(t));
        let s = r.ui.find_state();
        assert_eq!((s.readout.as_str(), s.can_step), ("", false));
        r.find_edited("zzz");
        // Still counting with nothing found: no "none" yet.
        let s = r.ui.find_state();
        assert_eq!((s.readout.as_str(), s.can_step), ("", false));
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
        r.settle_find();
        assert_eq!(r.ui.find_state().readout, "1000+");
    }

    #[test]
    fn closing_find_clears_highlights_and_the_selection_the_match_made() {
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
        assert!(
            !r.has_sel(),
            "the match's selection goes with the find bar, so Ctrl+C reaches the program"
        );
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
            shell_cwd: None,
            alive: true,
            remote: false,
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
            shell_cwd: None,
            alive: false,
            remote: false,
        };
        r.ui.on_dropped(Some(&dead), Path::new("C:\\a.txt"));
        r.ui.on_dropped(None, Path::new("C:\\b.txt"));
        r.tick(100);
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty());
    }

    /// Rule 304: a drop on a remote tab is never typed (this machine's paths mean nothing to the host's shell): the
    /// batch goes to the app as files to copy to the host first.
    #[test]
    fn a_drop_on_a_remote_tab_is_never_typed_and_goes_to_the_app_to_copy() {
        let mut r = Rig::new(1.0);
        r.remote = true;
        r.with_tab(|ui, t| {
            ui.on_dropped(Some(t), Path::new("C:\\My Dir\\a.txt"));
            ui.on_dropped(Some(t), Path::new("C:\\b.png"));
        });
        r.tick(100);
        let got: Vec<(String, Vec<String>, bool)> = r
            .msgs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| match m {
                TermMsg::RemoteUpload { tab, files, image } => {
                    Some((tab.clone(), files.clone(), *image))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            [(
                "a".to_string(),
                vec!["C:\\My Dir\\a.txt".to_string(), "C:\\b.png".to_string()],
                false
            )]
        );
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty(), "nothing typed into the pty");
    }

    /// Rule 304: on a remote tab copied files and an image are not pasted here (no local path, no image key): they go
    /// to the app to copy to the host. Text still pastes as text.
    #[test]
    fn a_paste_of_files_or_an_image_on_a_remote_tab_goes_to_the_app_to_copy() {
        let mut r = Rig::new(1.0);
        r.remote = true;
        let req = PasteReq {
            tab: "a".into(),
            mode: PasteMode::Normal,
        };
        let state = ClipboardState {
            files: vec!["C:\\x.txt".into()],
            text: String::new(),
            image: true,
        };
        r.with_tab(|ui, t| ui.on_msg(Some(t), TermMsg::Clip(req.clone(), state)));
        let image_only = ClipboardState {
            image: true,
            ..ClipboardState::default()
        };
        r.with_tab(|ui, t| ui.on_msg(Some(t), TermMsg::Clip(req.clone(), image_only)));
        let got: Vec<(Vec<String>, bool)> = r
            .msgs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| match m {
                TermMsg::RemoteUpload { files, image, .. } => Some((files.clone(), *image)),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            [(vec!["C:\\x.txt".to_string()], true), (Vec::new(), true)]
        );
        thread::sleep(Duration::from_millis(30));
        assert!(r.written().is_empty(), "no path and no image key typed");
    }

    /// Rule 1190: a remote tab chooses as a local tab does. An Excel copy (text plus a bitmap) pasted at a plain prompt
    /// pastes its text, Normal or Plain, and a Plain paste of copied files is their text only: nothing is copied to the
    /// host. With an agent armed the picture still goes to the host.
    #[test]
    fn a_remote_tab_pastes_the_text_of_an_excel_copy_and_a_plain_paste_never_copies_to_the_host() {
        let mut r = Rig::new(1.0);
        r.remote = true;
        let excel = ClipboardState {
            files: Vec::new(),
            text: "a\tb".into(),
            image: true,
        };
        let files = ClipboardState {
            files: vec!["C:\\x.txt".into()],
            text: "c".into(),
            image: false,
        };
        for (mode, state) in [
            (PasteMode::Normal, excel.clone()),
            (PasteMode::Plain, excel.clone()),
            (PasteMode::Plain, files),
        ] {
            let req = PasteReq {
                tab: "a".into(),
                mode,
            };
            r.with_tab(|ui, t| ui.on_msg(Some(t), TermMsg::Clip(req, state)));
        }
        let uploads = |r: &Rig| {
            r.msgs
                .lock()
                .unwrap()
                .iter()
                .filter(|m| matches!(m, TermMsg::RemoteUpload { .. }))
                .count()
        };
        assert_eq!(uploads(&r), 0, "nothing copied to the host");
        assert_eq!(r.wait_written(7), b"a\tba\tbc");
        r.agent.0.set(Some(AgentKind::Claude));
        let req = PasteReq {
            tab: "a".into(),
            mode: PasteMode::Normal,
        };
        r.with_tab(|ui, t| ui.on_msg(Some(t), TermMsg::Clip(req, excel)));
        assert_eq!(uploads(&r), 1, "an armed agent gets the picture");
    }

    // ---- OSC 52 and the program's own copy ----

    #[test]
    fn osc_52_from_the_front_tab_copies_and_badges() {
        let mut r = Rig::new(1.0);
        r.ui.on_term_event(
            "b",
            Some("b"),
            &TermEvent::ClipboardWrite("OSC52 \u{e6}".into()),
        );
        r.pump();
        assert_eq!(r.clip.read_text().as_deref(), Some("OSC52 \u{e6}"));
        assert_eq!(r.badge(), BadgePhase::Shown);
        assert!(r.written().is_empty(), "nothing is written back");
    }

    // Audit 2026-10-20 (L4): a background tab can no longer replace the clipboard (pastejacking).
    #[test]
    fn osc_52_from_a_background_tab_is_ignored() {
        let mut r = Rig::new(1.0);
        r.clip.set_text("mine");
        r.ui.on_term_event("b", Some("a"), &TermEvent::ClipboardWrite("evil".into()));
        r.ui.on_term_event("b", None, &TermEvent::ClipboardWrite("evil".into()));
        thread::sleep(Duration::from_millis(60));
        assert_eq!(r.clip.read_text().as_deref(), Some("mine"));
        assert!(r.msgs.lock().unwrap().is_empty(), "no copy was started");
        assert_eq!(r.badge(), BadgePhase::Gone);
    }

    // Audit 2026-10-20 (L39, L48): a flood runs on one worker and the last write wins.
    #[test]
    fn an_osc_52_flood_uses_one_worker_and_the_last_write_wins() {
        let probe = Arc::new(Probe::default());
        let mut r = Rig::with_port(1.0, probe.clone());
        for i in 0..200 {
            r.ui.on_term_event("a", Some("a"), &TermEvent::ClipboardWrite(format!("t{i}")));
        }
        until("the last write", || {
            probe.inner.read_text().as_deref() == Some("t199")
        });
        r.msgs.lock().unwrap().clear();
        let me = thread::current().id();
        let seen = probe.threads.lock().unwrap().clone();
        let mut ids: Vec<String> = seen.iter().map(|t| format!("{t:?}")).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 1, "{} threads wrote", ids.len());
        assert!(seen.iter().all(|t| *t != me));
    }

    /// A clipboard whose writes wait until the test lets them through.
    struct Gated {
        inner: MemoryClipboard,
        open: Mutex<bool>,
        cv: std::sync::Condvar,
    }
    impl Gated {
        fn new() -> Self {
            Self {
                inner: MemoryClipboard::new(),
                open: Mutex::new(false),
                cv: std::sync::Condvar::new(),
            }
        }
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.cv.notify_all();
        }
    }
    impl ClipboardPort for Gated {
        fn write_text(&self, t: &str, m: usize) -> Result<(), String> {
            let mut g = self.open.lock().unwrap();
            while !*g {
                g = self.cv.wait(g).unwrap();
            }
            drop(g);
            self.inner.write_text(t, m)
        }
        fn read_text(&self) -> Option<String> {
            self.inner.read_text()
        }
        fn read_state(&self) -> ClipboardState {
            self.inner.read_state()
        }
        fn sequence(&self) -> u32 {
            self.inner.sequence()
        }
    }

    // Audit 2026-10-20 (L49): the pending big copy belongs to its tab, and closing the tab frees it.
    #[test]
    fn a_pending_big_copy_belongs_to_its_tab_and_a_closed_tab_frees_it() {
        let gate = Arc::new(Gated::new());
        let mut r = Rig::with_port(1.0, gate.clone());
        r.feed(&(0..300).map(|i| format!("row {i}\r\n")).collect::<String>());
        select_everything(&r);
        r.key(ctrl("c", "KeyC"));
        until("the copy is pending", || r.ui.copy_is_pending("a"));
        assert!(!r.ui.copy_is_pending("b"), "another tab is not copying");
        r.ui.tab_closed("a");
        assert!(
            !r.ui.copy_is_pending("a"),
            "a closed tab cannot stick the flag"
        );
        gate.release();
        r.pump();
    }

    #[test]
    fn osc_52_failure_raises_no_badge() {
        let mut r = Rig::with_port(1.0, Arc::new(Refusing));
        r.ui.on_term_event("a", Some("a"), &TermEvent::ClipboardWrite("x".into()));
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
    fn a_failed_copy_raises_no_badge() {
        // Parity line 48: the badge is raised only by a successful copy.
        let mut r = Rig::new(1.0);
        r.ui.on_msg(None, TermMsg::Copied(false));
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
        r.ui.on_term_event("a", Some("a"), &TermEvent::ClipboardWrite("x".into()));
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
