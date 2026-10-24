//! PanelCtl: the panel of each tab, and what the window shows of it (Surfaces stage 1).
//!
//! The model is `flare_surfaces::TabPanels` (a panel per tab, its kind, target and share of the
//! page), stored in `panels.json`. This controller owns it on the UI thread, pushes the front tab's
//! panel into the window's `panel-*` properties and reads the file or the picture on a worker
//! (rule 34): the answer comes back as a `UiMsg::Panel` and a number says which request it answers,
//! so a slow read of a file the user has since left is dropped.
//!
//! A closed panel costs nothing: no worker, no picture, no rows and no timer; the properties hold
//! empty values. Only the front tab's panel is loaded; a tab behind keeps its path and reads it
//! again when it comes to the front.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use flare_surfaces::PanelKind;
use flare_surfaces::app::{self, AppSpec};
use flare_surfaces::preview::{self, Cut, Decoded, LINES_MAX, Reason, TEXT_CAP};
use flare_surfaces::state::TabPanels;
use flare_surfaces::store::{PanelStore, SavedTabPanel, is_blank, match_restored};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::AppWindow;
use crate::appctl::{Post, UiMsg};

/// A worker's answer for the panel controller.
pub enum PanelMsg {
    Image {
        generation: u64,
        tab: String,
        result: Result<LoadedImage, Reason>,
    },
    Text {
        generation: u64,
        tab: String,
        result: Result<TextRows, Reason>,
    },
    /// A big file wrapped to `cols` on a worker (a small one is wrapped where it arrives). `seq` says which request
    /// it answers: only the newest is used.
    Wrapped {
        generation: u64,
        tab: String,
        seq: u64,
        cols: usize,
        rows: Vec<SharedString>,
        nums: Vec<i32>,
    },
}

/// Text under this many bytes is wrapped on the UI thread where it arrives (a few milliseconds at most); a bigger
/// one is wrapped on a worker, and the panel says "Opening" until it answers.
const WRAP_INLINE_MAX: usize = 1_000_000;
/// While the width is moving, a file over this many bytes is wrapped on a worker, not where the step lands.
const WRAP_LIVE_INLINE_MAX: usize = 200_000;

/// A text file ready for the window: the lines are shaped on the worker, so the UI thread only wraps
/// them in a model.
pub struct TextRows {
    pub lines: Arc<Vec<SharedString>>,
    /// The widest row, in screen columns.
    pub cols: usize,
    /// The file's real length.
    pub bytes: u64,
    /// The foot's words when a cap cut the file short, else empty.
    pub foot: String,
}

impl TextRows {
    pub fn from_doc(doc: preview::TextDoc) -> Self {
        // The widest line in screen columns: a wide letter or an emoji takes two, so counting characters would leave
        // the right half of such a line out of the scroll range.
        let cols = doc
            .lines
            .iter()
            .map(|l| flare_surfaces::wrap::display_cols(l))
            .max()
            .unwrap_or(0);
        let foot = match doc.cut {
            Some(Cut::Bytes) => format!("Only the first {} are shown.", human_bytes(TEXT_CAP)),
            Some(Cut::Lines) => format!("Only the first {LINES_MAX} lines are shown."),
            None => String::new(),
        };
        Self {
            lines: Arc::new(doc.lines.into_iter().map(SharedString::from).collect()),
            cols,
            bytes: doc.bytes,
            foot,
        }
    }
}

/// One picture is decoded at a time: a decode at the pixel limit holds hundreds of megabytes, so a
/// second one waits (and is dropped unread when the user has moved on by then).
static DECODE: Mutex<()> = Mutex::new(());

/// A decoded picture, already in the buffer Slint draws from (made on the worker, so the UI thread
/// copies nothing).
pub struct LoadedImage {
    pub buf: SharedPixelBuffer<Rgba8Pixel>,
    pub has_alpha: bool,
    pub scaled: bool,
}

/// A `PanelMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never a path.
pub struct PanelMsgBox(pub PanelMsg);

impl std::fmt::Debug for PanelMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            PanelMsg::Image { .. } => "PanelMsg::Image",
            PanelMsg::Text { .. } => "PanelMsg::Text",
            PanelMsg::Wrapped { .. } => "PanelMsg::Wrapped",
        })
    }
}

/// What the window shows of one tab's panel now.
enum Content {
    Reading,
    Failed(String),
    Image {
        image: Image,
        w: u32,
        h: u32,
        alpha: bool,
        scaled: bool,
    },
    Text {
        /// The file's lines, one per line (kept while the panel shows, for a re-wrap).
        lines: Arc<Vec<SharedString>>,
        /// What the window draws: the lines, or the rows they wrap to.
        shown: ModelRc<SharedString>,
        /// The line number of each shown row (0: a continuation); empty while the rows are the lines.
        nums: ModelRc<i32>,
        count: usize,
        cols: usize,
        foot: String,
        bytes: u64,
        /// The shaped text's size (the lines' bytes, tabs already spaces): what the live wrap judges, as `is_big` does.
        shaped: usize,
        /// The columns the shown rows are wrapped to; 0 while they are the lines.
        wrapped_cols: usize,
        /// False while a big file waits for its first wrap (the panel says "Opening").
        ready: bool,
    },
}

/// Where the reader was in a tab's file when the tab left the front: the line at the top of a text view, and whether a
/// picture was at its real size. Kept for the tabs that left (a few), and used once, when the same file comes back.
struct ViewMemory {
    target: String,
    line: usize,
    actual: bool,
}

/// How many tabs' places are kept.
const MEMORY_CAP: usize = 8;

struct Shown {
    tab: String,
    kind: PanelKind,
    target: String,
    generation: u64,
    content: Content,
}

/// What a panel's target starts with when the content was handed over as text (`view.text`), not read
/// from a file. Such a target never loads from disk, is never written to `panels.json` and is never
/// restored: the text lives in memory for the session only.
pub const INLINE: &str = "inline:";

/// The text a tab's panel shows, with the target that names it (`inline:<n>`).
struct InlineText {
    target: String,
    text: Arc<str>,
}

/// What the control channel reports of a tab's panel (`PanelInfo`, `panel.state`).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelSnap {
    /// The kind's name on the wire: `image`, `text`, ...
    pub kind: String,
    pub title: String,
    /// The file shown; None for text that was handed over and for a web page.
    pub path: Option<String>,
    /// The address of a web panel's page (none for a mini-app: its address is Flare's own).
    pub url: Option<String>,
    /// For a mini-app: whether its page may load from the web (the agent asked and the user's switch
    /// was on). None for every other panel.
    pub network: Option<bool>,
    pub open: bool,
    /// The slot's share of the page the user dragged it to, if they did.
    pub width: Option<f32>,
}

pub struct PanelCtl {
    panels: TabPanels,
    /// The mini-app each tab shows, if any (stage 5). Its panel is a web panel at the app's address;
    /// it lives for the session only (never stored, never restored) and its folder goes with it.
    apps: HashMap<String, AppSpec>,
    /// The mod each tab shows, if any (stage 8). A mod's panel is a web panel at the mod's own address; it lives
    /// for the session (a mod must be on first, so it is never stored or restored) and, unlike an app, its folder
    /// is the user's own and is never touched.
    mods: HashMap<String, AppSpec>,
    /// Where the apps' folders are (`<profile>\apps`); a folder is removed only from here.
    apps_root: Option<std::path::PathBuf>,
    /// A title a caller gave (the header shows it instead of the file's name), by tab.
    titles: HashMap<String, String>,
    /// Where the reader was in the files of tabs that went behind, by tab.
    memory: HashMap<String, ViewMemory>,
    /// A refused "Show in side panel" on a tab whose panel is open: its words sit in the header's second line until the
    /// tab shows something else (the panel itself is left as it was).
    refused: HashMap<String, String>,
    /// Text handed over for a tab's panel, by tab.
    inline: HashMap<String, InlineText>,
    inline_seq: u64,
    store: Option<Arc<PanelStore>>,
    /// The entries the file held at start, until the restore gives them to the tabs.
    boot: Vec<SavedTabPanel>,
    /// What was last handed to the store (None: nothing yet, so a session with no panels writes
    /// no file).
    saved: Option<Vec<SavedTabPanel>>,
    /// The file held at least one panel at start: a session that ends with none must say so.
    file_had_panels: bool,
    shown: Option<Shown>,
    generation: u64,
    /// The newest generation, shared with the workers: one that is not it stops at its next look.
    current: Arc<AtomicU64>,
    /// The same file was shown again (a click on the path once more): read it afresh.
    reload: bool,
    post: Post,
    checker: Option<Image>,
    /// "Wrap long lines in previews" (the setting; the controller of the window keeps it current).
    wrap: bool,
    /// The newest wrap request, shared with the workers: one that is not it stops at its next look.
    wrap_seq: u64,
    wrap_current: Arc<AtomicU64>,
    /// The columns of the wrap on its way from a worker (0: none).
    wrap_pending: usize,
    /// The width is still moving (a window edge or splitter step, until the settle timer fires): a refresh then
    /// wraps live, never the exact way.
    wrap_moving: bool,
    /// How many wraps were made (inline or on a worker): the tests hold that a drag makes none and its release one.
    wraps: u64,
}

/// The checkerboard tile: 2 by 2 squares of 8 px, two neutral greys that read on any theme.
pub(crate) fn checker_tile() -> Image {
    const SIDE: u32 = 16;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(SIDE, SIDE);
    let px = buf.make_mut_slice();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let light = ((x / 8) + (y / 8)) % 2 == 0;
            let g = if light { 0x9a } else { 0x7a };
            px[(y * SIDE + x) as usize] = Rgba8Pixel {
                r: g,
                g,
                b: g,
                a: 255,
            };
        }
    }
    Image::from_rgba8(buf)
}

/// The last part of a path, for the header.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether a panel of `kind` may point at `target`: a file on a local drive of the right kind, or
/// (a web panel) an address exactly as it normalises, so a stored one is vetted like a typed one.
fn target_ok(kind: &PanelKind, target: &str) -> bool {
    match kind {
        PanelKind::Web => flare_surfaces::web::normalize_url(target).as_deref() == Ok(target),
        // The conversation panels read no file: their target is the kind's own name, nothing else.
        PanelKind::Changes | PanelKind::Tasks => kind.fixed_target() == Some(target),
        _ => preview::loadable(target, kind),
    }
}

/// The header of a panel that reads no file: the conversation's two.
fn convo_title(kind: &PanelKind) -> &'static str {
    match kind {
        PanelKind::Changes => "Changes",
        _ => "Tasks",
    }
}

/// "12 KB", "3.4 MB": plain units, one decimal from a megabyte up.
pub fn human_bytes(n: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    if n < KB {
        format!("{n} B")
    } else if n < MB {
        format!("{} KB", n.div_ceil(KB))
    } else {
        format!("{:.1} MB", n as f64 / MB as f64)
    }
}

impl PanelCtl {
    pub fn new(post: Post) -> Self {
        Self {
            panels: TabPanels::default(),
            apps: HashMap::new(),
            mods: HashMap::new(),
            apps_root: None,
            titles: HashMap::new(),
            refused: HashMap::new(),
            memory: HashMap::new(),
            inline: HashMap::new(),
            inline_seq: 0,
            store: None,
            boot: Vec::new(),
            saved: None,
            file_had_panels: false,
            shown: None,
            generation: 0,
            current: Arc::new(AtomicU64::new(0)),
            reload: false,
            post,
            checker: None,
            wrap: true,
            wrap_seq: 0,
            wrap_current: Arc::new(AtomicU64::new(0)),
            wrap_pending: 0,
            wrap_moving: false,
            wraps: 0,
        }
    }

    /// Hands over the store and reads the file (startup, before any window shows).
    pub fn set_store(&mut self, store: Arc<PanelStore>) {
        self.boot = store.load(); // sync-ok: startup read of panels.json
        self.file_had_panels = !is_blank(&self.boot);
        self.store = Some(store);
    }

    /// The restore has made the tabs: each takes the panel its folder and place in the strip name.
    pub fn restore(&mut self, tabs: &[(String, String)]) {
        let cwds: Vec<String> = tabs.iter().map(|t| t.1.clone()).collect();
        let found = match_restored(&self.boot, &cwds);
        self.boot.clear();
        for ((id, _), panel) in tabs.iter().zip(found) {
            // A restored target is checked as a click's is: the file is anyone's to edit.
            if let Some(p) = panel
                && (!p.kind.is_built() || target_ok(&p.kind, &p.target))
                && !app::is_app_url(&p.target)
                && !flare_surfaces::modview::is_mod_url(&p.target)
            {
                self.panels.restore(id, p);
            }
        }
        // What was read is what is on disk: the first persist writes only a real change.
        self.saved = None;
    }

    /// Shows `kind` / `target` on the tab and opens it. False for a target the panel will not load.
    pub fn show(&mut self, tab: &str, kind: PanelKind, target: &str) -> bool {
        if kind.is_built() && !target_ok(&kind, target) {
            return false;
        }
        // Whatever the tab shows now replaces a mini-app it had.
        self.drop_app(tab);
        // The file may have changed since it was read, or failed and now exist: a click on what
        // is on show reads it again.
        if self.shown.as_ref().is_some_and(|s| s.tab == tab) {
            self.reload = true;
        }
        // A click or a restore shows the file under its own name: a title an agent gave an earlier
        // call, and text it handed over, end with what they were for.
        self.titles.remove(tab);
        self.inline.remove(tab);
        self.refused.remove(tab);
        self.panels.show(tab, kind, target)
    }

    /// A "Show in side panel" the file could not honour, on a tab whose panel is open: the panel is left as it was
    /// and the words go in its header. False when the tab has no open panel (the caller shows them as a note).
    pub fn refuse(&mut self, app: &AppWindow, tab: &str, text: &str) -> bool {
        if self.panels.open_on(tab).is_none() {
            return false;
        }
        self.refused.insert(tab.to_string(), text.to_string());
        if self.shown.as_ref().is_some_and(|s| s.tab == tab) {
            self.push(app);
        }
        true
    }

    pub fn close(&mut self, tab: &str) -> bool {
        // A mini-app has nothing to come back to: closing it is the end of it (and of its folder). A mod's
        // panel is the same: it is shown again by the mod, not brought back from a stored address.
        if self.apps.contains_key(tab) || self.mods.contains_key(tab) {
            let was_open = self.panels.open_on(tab).is_some();
            self.drop_app(tab);
            self.panels.forget(tab);
            self.titles.remove(tab);
            return was_open;
        }
        self.panels.close(tab)
    }

    /// Where the apps' folders are (startup).
    pub fn set_apps_root(&mut self, root: std::path::PathBuf) {
        self.apps_root = Some(root);
    }

    /// Ends the tab's mini-app, if it has one: its folder is removed on a worker (rule 34). A mod the tab showed
    /// is let go too, and its folder is left alone.
    fn drop_app(&mut self, tab: &str) {
        self.mods.remove(tab);
        if let Some(spec) = self.apps.remove(tab) {
            self.remove_folder(spec);
        }
    }

    /// Removes an app's folder on a worker thread; only a folder directly inside the apps root goes.
    pub fn remove_folder(&self, spec: AppSpec) {
        let Some(root) = self.apps_root.clone() else {
            return;
        };
        let job = move || {
            app::remove_app(&root, &spec.folder);
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-app-clean".into())
            .spawn(job)
        {
            log::warn!("could not start the mini-app cleaner: {e}");
        }
    }

    /// Shows a mini-app on the tab: a web panel at the app's address under the caller's title. A mini-app
    /// the tab had goes first. False for an address the panel cannot hold.
    pub fn show_app(&mut self, tab: &str, spec: AppSpec, title: &str) -> bool {
        self.drop_app(tab);
        self.titles.remove(tab);
        self.inline.remove(tab);
        if self.shown.as_ref().is_some_and(|s| s.tab == tab) {
            self.reload = true;
        }
        if !self.panels.show(tab, PanelKind::Web, &spec.url) {
            self.remove_folder(spec);
            return false;
        }
        self.titles.insert(tab.to_string(), title.to_string());
        self.apps.insert(tab.to_string(), spec);
        true
    }

    /// Shows a mod's panel on the tab (stage 8): a web panel at the mod's own address under the manifest's title.
    /// What the tab showed goes first. False for an address the panel cannot hold.
    pub fn show_mod(&mut self, tab: &str, spec: AppSpec, title: &str) -> bool {
        self.drop_app(tab);
        self.titles.remove(tab);
        self.inline.remove(tab);
        if self.shown.as_ref().is_some_and(|s| s.tab == tab) {
            self.reload = true;
        }
        if !self.panels.show(tab, PanelKind::Web, &spec.url) {
            return false;
        }
        self.titles.insert(tab.to_string(), title.to_string());
        self.mods.insert(tab.to_string(), spec);
        true
    }

    /// The mod the tab's panel shows (open or not).
    pub fn mod_of(&self, tab: &str) -> Option<&str> {
        self.mods
            .get(tab)
            .and_then(|s| s.module.as_ref())
            .map(|m| m.id.as_str())
    }

    /// The tabs that show a mod, with the mod's id.
    pub fn mod_tabs(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .mods
            .iter()
            .filter_map(|(t, s)| s.module.as_ref().map(|m| (t.clone(), m.id.clone())))
            .collect();
        v.sort();
        v
    }

    /// The mini-app or mod the tab shows (open or not).
    pub fn app_spec(&self, tab: &str) -> Option<&AppSpec> {
        self.apps.get(tab).or_else(|| self.mods.get(tab))
    }

    /// The tab shows a page that is not the person's to point anywhere: a mini-app or a mod (no address bar).
    pub fn is_app(&self, tab: &str) -> bool {
        self.apps.contains_key(tab) || self.mods.contains_key(tab)
    }

    pub fn reopen(&mut self, tab: &str) -> bool {
        self.panels.reopen(tab)
    }

    pub fn is_open(&self, tab: &str) -> bool {
        self.panels.open_on(tab).is_some()
    }

    /// The agent in the tab opened what its panel shows (a control channel call): the panel closes when that
    /// agent leaves.
    pub fn mark_agent(&mut self, tab: &str) {
        self.panels.mark_agent(tab);
    }

    /// The tabs whose open panel their agent opened.
    pub fn agent_tabs(&self) -> Vec<String> {
        self.panels.agent_tabs()
    }

    /// The agent that opened the tab's panel has left: the panel closes as its close button would, and what it
    /// showed is the user's from then on. False when the panel was not the agent's or was not open.
    pub fn close_for_agent(&mut self, tab: &str) -> bool {
        if !self.panels.unmark_agent(tab) {
            return false;
        }
        self.close(tab)
    }

    /// Opens the Changes or Tasks panel on the tab (the conversation's, stage 6).
    /// False for any other kind.
    pub fn show_convo(&mut self, tab: &str, kind: PanelKind) -> bool {
        match kind.fixed_target() {
            Some(target) => self.show(tab, kind, target),
            None => false,
        }
    }

    /// The tabs whose open panel is Changes or Tasks, with the kind: the tabs that need a transcript tracker.
    pub fn open_convo(&self) -> Vec<(String, PanelKind)> {
        self.panels.open_convo()
    }

    /// The kind of the tab's OPEN panel when it is Changes or Tasks.
    pub fn open_convo_kind(&self, tab: &str) -> Option<PanelKind> {
        self.panels
            .open_on(tab)
            .map(|p| p.kind.clone())
            .filter(PanelKind::is_convo)
    }

    /// The tab has a panel (open or closed) of a kind this build draws.
    pub fn has_built(&self, tab: &str) -> bool {
        self.panels.get(tab).is_some_and(|p| p.kind.is_built())
    }

    /// The address of the tab's OPEN web panel.
    pub fn web_target(&self, tab: &str) -> Option<String> {
        self.panels
            .open_on(tab)
            .filter(|p| p.kind == PanelKind::Web)
            .map(|p| p.target.clone())
    }

    /// The page of the tab's web panel moved: the panel points at it from now on (it is what a
    /// restart and the toggle chord bring back). Only an address a panel may open is kept, in its
    /// normal form.
    pub fn set_web_url(&mut self, tab: &str, url: &str) -> bool {
        // A mini-app stays at its own address (the browser cancels every other navigation).
        if self.is_app(tab) {
            return false;
        }
        let Ok(url) = flare_surfaces::web::normalize_url(url) else {
            return false;
        };
        if self.panels.get(tab).map(|p| &p.kind) != Some(&PanelKind::Web) {
            return false;
        }
        self.panels.set_target(tab, &url)
    }

    /// The page of the tab's web panel has a title: the channel names the panel by it (an empty one
    /// goes back to the host's name).
    pub fn set_web_title(&mut self, tab: &str, title: &str) -> bool {
        // The title the agent gave a mini-app is the panel's name, whatever the page calls itself.
        if self.is_app(tab) {
            return false;
        }
        if self.panels.get(tab).map(|p| &p.kind) != Some(&PanelKind::Web) {
            return false;
        }
        let one_line: String = title
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(200)
            .collect();
        let one_line = one_line.trim();
        if one_line.is_empty() {
            self.titles.remove(tab).is_some()
        } else {
            self.titles
                .insert(tab.to_string(), one_line.to_string())
                .as_deref()
                != Some(one_line)
        }
    }

    pub fn forget(&mut self, tab: &str) {
        self.refused.remove(tab);
        self.memory.remove(tab);
        self.drop_app(tab);
        self.panels.forget(tab);
        self.titles.remove(tab);
        self.inline.remove(tab);
    }

    /// Shows a file on the tab like [`PanelCtl::show`], under a caller's title (None: the file's name).
    pub fn show_titled(
        &mut self,
        tab: &str,
        kind: PanelKind,
        target: &str,
        title: Option<&str>,
    ) -> bool {
        if !self.show(tab, kind, target) {
            return false;
        }
        match title {
            Some(t) => {
                self.titles.insert(tab.to_string(), t.to_string());
            }
            None => {
                self.titles.remove(tab);
            }
        }
        true
    }

    /// Shows a piece of text on the tab (`view.text`). Each call is a new target, so the panel reads
    /// it afresh. False for a tab that cannot hold a panel target.
    pub fn show_text(&mut self, tab: &str, text: &str, title: Option<&str>) -> bool {
        self.inline_seq += 1;
        let target = format!("{INLINE}{}", self.inline_seq);
        self.drop_app(tab);
        if !self.panels.show(tab, PanelKind::Text, &target) {
            return false;
        }
        self.inline.insert(
            tab.to_string(),
            InlineText {
                target,
                text: Arc::from(text),
            },
        );
        self.titles
            .insert(tab.to_string(), title.unwrap_or("Text").to_string());
        true
    }

    /// What the control channel says of the tab's panel; None when it has none (open or closed).
    pub fn snap(&self, tab: &str) -> Option<PanelSnap> {
        let p = self.panels.get(tab)?;
        let inline = p.target.starts_with(INLINE);
        let app = self.apps.get(tab);
        let module = self.mods.get(tab).and_then(|s| s.module.as_ref());
        let web = p.kind == PanelKind::Web && app.is_none() && module.is_none();
        let convo = p.kind.is_convo();
        Some(PanelSnap {
            kind: if let Some(m) = module {
                PanelKind::Mod(m.id.clone()).name()
            } else if app.is_some() {
                "app".to_string()
            } else {
                p.kind.name()
            },
            network: app.map(|a| a.allow_network),
            title: self.titles.get(tab).cloned().unwrap_or_else(|| {
                if web {
                    flare_surfaces::web::host_label(&p.target)
                } else if convo {
                    convo_title(&p.kind).to_string()
                } else {
                    file_name(&p.target).to_string()
                }
            }),
            path: (!inline && !web && !convo && app.is_none() && module.is_none())
                .then(|| p.target.clone()),
            url: web.then(|| p.target.clone()),
            open: p.open,
            width: p.width,
        })
    }

    pub fn set_share(&mut self, tab: &str, share: f32) -> bool {
        self.panels.set_width(tab, share)
    }

    /// How many panels are open on any tab (a closed one costs nothing).
    pub fn open_count(&self) -> usize {
        self.panels.open_count()
    }

    /// Writes the panels of these tabs (id and folder, in the strip's order) when they changed.
    pub fn persist(&mut self, tabs: &[(String, String)]) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let list: Vec<SavedTabPanel> = tabs
            .iter()
            .map(|(id, cwd)| SavedTabPanel {
                cwd: cwd.clone(),
                // Text that was handed over, and a mini-app, live in memory for this run only.
                panel: self
                    .panels
                    .get(id)
                    .filter(|p| !p.target.starts_with(INLINE) && !self.is_app(id))
                    .cloned(),
            })
            .collect();
        let blank = is_blank(&list);
        // Nothing to say and nothing on disk to correct: no file, and no writer thread, for a
        // session that never opens a panel.
        if blank && !self.file_had_panels {
            return;
        }
        if self.saved.as_ref() == Some(&list) {
            return;
        }
        store.save(&list);
        self.file_had_panels = !blank;
        self.saved = Some(list);
    }

    pub fn flush(&self) {
        if let Some(s) = &self.store {
            s.flush();
        }
    }

    /// The window shows the front tab's panel (or none). Starts the read when the front panel is not
    /// the one on show.
    pub fn sync(&mut self, app: &AppWindow, front: Option<&str>) {
        let open = front.and_then(|t| self.panels.open_on(t).filter(|p| p.kind.is_built()));
        app.set_panel_open(open.is_some());
        app.set_panel_share(open.and_then(|p| p.width).unwrap_or(0.0));
        app.set_panel_wrap_on(self.wrap);
        // A web panel is not read from disk: the web controller owns its content (and the window's
        // `panel-*` properties for it).
        let want = front.and_then(|t| {
            self.panels
                .open_on(t)
                .filter(|p| p.kind.is_built() && p.kind != PanelKind::Web && !p.kind.is_convo())
                .map(|p| (t.to_string(), p.kind.clone(), p.target.clone()))
        });
        let Some((tab, kind, target)) = want else {
            if self.shown.is_some() {
                self.remember(app);
                self.shown = None;
                self.generation += 1;
                self.current.store(self.generation, Ordering::SeqCst);
                self.clear(app);
            }
            self.reload = false;
            return;
        };
        let same = self
            .shown
            .as_ref()
            .is_some_and(|s| s.tab == tab && s.kind == kind && s.target == target);
        if same && !std::mem::take(&mut self.reload) {
            // The width or the setting may have changed since the rows were made.
            if self.wrap_moving {
                self.live_wrap(app);
            } else {
                self.settle_wrap(app);
            }
            return;
        }
        self.reload = false;
        // Another tab's file comes to the front: where the reader was in the one that leaves is kept for its return.
        if !same && self.shown.as_ref().is_some_and(|s| s.tab != tab) {
            self.remember(app);
        }
        self.generation += 1;
        let generation = self.generation;
        self.current.store(generation, Ordering::SeqCst);
        if same {
            // What is on show stays until the new read answers (no flash of "Opening").
            if let Some(s) = self.shown.as_mut() {
                s.generation = generation;
            }
        } else {
            self.shown = Some(Shown {
                tab: tab.clone(),
                kind: kind.clone(),
                target: target.clone(),
                generation,
                content: Content::Reading,
            });
            self.push(app);
        }
        self.spawn_load(&tab, &kind, &target, generation);
    }

    /// A worker's answer. Dropped when it is for a file no longer on show.
    pub fn on_msg(&mut self, app: &AppWindow, msg: PanelMsg) {
        let Some(shown) = self.shown.as_ref() else {
            return;
        };
        let (generation, tab) = match &msg {
            PanelMsg::Image {
                generation, tab, ..
            }
            | PanelMsg::Text {
                generation, tab, ..
            }
            | PanelMsg::Wrapped {
                generation, tab, ..
            } => (*generation, tab.clone()),
        };
        if generation != shown.generation || tab != shown.tab {
            return;
        }
        let content = match msg {
            PanelMsg::Image { result, .. } => match result {
                Ok(l) => Content::Image {
                    w: l.buf.width(),
                    h: l.buf.height(),
                    image: Image::from_rgba8(l.buf),
                    alpha: l.has_alpha,
                    scaled: l.scaled,
                },
                Err(r) => Content::Failed(r.to_string()),
            },
            PanelMsg::Text { result, .. } => match result {
                Ok(t) => self.text_content(app, &tab, generation, t),
                Err(r) => Content::Failed(r.to_string()),
            },
            PanelMsg::Wrapped {
                seq,
                cols,
                rows,
                nums,
                ..
            } => {
                if seq != self.wrap_seq {
                    return;
                }
                self.wrap_pending = 0;
                let top = self.top_line(app);
                if let Some(Shown {
                    content:
                        Content::Text {
                            shown,
                            nums: n,
                            wrapped_cols,
                            ready,
                            ..
                        },
                    ..
                }) = self.shown.as_mut()
                {
                    *shown = ModelRc::from(Rc::new(VecModel::from(rows)));
                    *n = ModelRc::from(Rc::new(VecModel::from(nums)));
                    *wrapped_cols = cols;
                    *ready = true;
                }
                self.push(app);
                self.restore_top(app, top);
                self.apply_memory(app);
                // The width may have moved while the wrap was on its way (a drag goes on while it is made).
                self.live_wrap(app);
                return;
            }
        };
        if let Some(s) = self.shown.as_mut() {
            s.content = content;
        }
        self.push(app);
        self.apply_memory(app);
    }

    /// Keeps the reader's place in the file on show (a text view's top line, a picture's real-size state) for the
    /// tab that is leaving the front. Only a file that was read and shown counts.
    fn remember(&mut self, app: &AppWindow) {
        let Some(s) = self.shown.as_ref() else { return };
        let (line, actual) = match &s.content {
            Content::Text { ready: true, .. } => (self.top_line(app).unwrap_or(1), false),
            Content::Image { .. } => (1, app.get_panel_actual()),
            _ => return,
        };
        let (tab, target) = (s.tab.clone(), s.target.clone());
        if self.memory.len() >= MEMORY_CAP && !self.memory.contains_key(&tab) {
            // Not an LRU: the cap only keeps a long session from holding a place for every tab it ever had.
            if let Some(k) = self.memory.keys().next().cloned() {
                self.memory.remove(&k);
            }
        }
        self.memory.insert(
            tab,
            ViewMemory {
                target,
                line,
                actual,
            },
        );
    }

    /// The file that was just shown is the one this tab left behind: the reader's place comes back, once.
    fn apply_memory(&mut self, app: &AppWindow) {
        let Some(s) = self.shown.as_ref() else { return };
        let ready = match &s.content {
            Content::Text { ready, .. } => *ready,
            Content::Image { .. } => true,
            _ => return,
        };
        if !ready {
            return;
        }
        let Some(mem) = self.memory.remove(&s.tab) else {
            return;
        };
        if mem.target != s.target {
            return;
        }
        match &s.content {
            Content::Text { .. } => self.restore_top(app, Some(mem.line)),
            _ => app.set_panel_actual(mem.actual),
        }
    }

    /// The columns the window's text view holds now, when previews are wrapped and the slot is on show; else 0.
    fn want_cols(&self, app: &AppWindow) -> usize {
        if !self.wrap || !app.get_panel_shown() {
            return 0;
        }
        usize::try_from(app.get_panel_wrap_cols()).unwrap_or(0)
    }

    /// The line (1-based) at the top of the text view now, for a re-wrap to keep; None when no text is on show.
    fn top_line(&self, app: &AppWindow) -> Option<usize> {
        let Some(Shown {
            content: Content::Text {
                nums, ready: true, ..
            },
            ..
        }) = self.shown.as_ref()
        else {
            return None;
        };
        let mut row = usize::try_from(app.get_panel_first_line()).unwrap_or(0);
        if nums.row_count() == 0 {
            return Some(row + 1);
        }
        row = row.min(nums.row_count() - 1);
        // A continuation row carries 0: its line starts on a row above.
        loop {
            let n = nums.row_data(row).unwrap_or(0);
            if n > 0 {
                return usize::try_from(n).ok();
            }
            if row == 0 {
                return Some(1);
            }
            row -= 1;
        }
    }

    /// After a re-wrap: the view's top goes back to the line it showed (the first row of that line).
    fn restore_top(&self, app: &AppWindow, line: Option<usize>) {
        let (
            Some(line),
            Some(Shown {
                content: Content::Text { nums, .. },
                ..
            }),
        ) = (line, self.shown.as_ref())
        else {
            return;
        };
        let row = if nums.row_count() == 0 {
            line.saturating_sub(1)
        } else {
            (0..nums.row_count())
                .find(|&r| usize::try_from(nums.row_data(r).unwrap_or(0)).is_ok_and(|n| n == line))
                .unwrap_or(0)
        };
        app.invoke_panel_scroll_to_row(i32::try_from(row).unwrap_or(0));
    }

    /// The newest wrap request is a new one: every older worker stops at its next look.
    fn next_wrap_seq(&mut self) -> u64 {
        self.wrap_seq += 1;
        self.wrap_current.store(self.wrap_seq, Ordering::SeqCst);
        self.wrap_pending = 0;
        self.wrap_seq
    }

    /// A text file arrived: its lines, wrapped to the width now when wrapping is on (a small file here, a big one on
    /// a worker while the panel says "Opening").
    fn text_content(
        &mut self,
        app: &AppWindow,
        tab: &str,
        generation: u64,
        t: TextRows,
    ) -> Content {
        // The gutter's width depends on the line count, so it is told before the columns are read.
        app.set_panel_number_count(i32::try_from(t.lines.len()).unwrap_or(i32::MAX));
        let want = self.want_cols(app);
        let count = t.lines.len();
        let shaped = shaped_len(&t.lines);
        let (shown, nums, wrapped_cols, ready) = if want == 0 {
            self.next_wrap_seq();
            (line_model(&t.lines), no_nums(), 0, true)
        } else if is_big(&t.lines) {
            self.spawn_wrap(tab, generation, Arc::clone(&t.lines), want);
            (ModelRc::default(), no_nums(), 0, false)
        } else {
            self.next_wrap_seq();
            self.wraps += 1;
            let (rows, nums) = wrap_lines(&t.lines, want, &|| false);
            (
                ModelRc::from(Rc::new(VecModel::from(rows))),
                ModelRc::from(Rc::new(VecModel::from(nums))),
                want,
                true,
            )
        };
        Content::Text {
            lines: t.lines,
            shown,
            nums,
            count,
            cols: t.cols,
            foot: t.foot,
            bytes: t.bytes,
            shaped,
            wrapped_cols,
            ready,
        }
    }

    /// Wraps the lines to `cols` on a worker; the answer is a `PanelMsg::Wrapped`.
    fn spawn_wrap(
        &mut self,
        tab: &str,
        generation: u64,
        lines: Arc<Vec<SharedString>>,
        cols: usize,
    ) {
        let seq = self.next_wrap_seq();
        self.wraps += 1;
        self.wrap_pending = cols;
        let current = Arc::clone(&self.wrap_current);
        let post = Arc::clone(&self.post);
        let tab = tab.to_string();
        let job = move || {
            let stale = || current.load(Ordering::SeqCst) != seq;
            let (rows, nums) = wrap_lines(&lines, cols, &stale);
            if !stale() {
                post(UiMsg::Panel(PanelMsgBox(PanelMsg::Wrapped {
                    generation,
                    tab,
                    seq,
                    cols,
                    rows,
                    nums,
                })));
            }
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-panel-wrap".into())
            .spawn(job)
        {
            log::warn!("could not start the wrap: {e}");
            self.wrap_pending = 0;
        }
    }

    /// The newest load's number (the tests build an answer for it).
    #[cfg(test)]
    pub fn generation_for_test(&self) -> u64 {
        self.generation
    }

    /// How many wraps were made so far (a request counts once, whoever does the work).
    pub fn wrap_count(&self) -> u64 {
        self.wraps
    }

    /// The width is moving (true: a step arrived) or has settled (false: the timer fired, the exact wrap is next).
    pub fn set_wrap_moving(&mut self, moving: bool) {
        self.wrap_moving = moving;
    }

    /// The setting "Wrap long lines in previews" as the window last had it.
    pub fn set_wrap(&mut self, on: bool) {
        self.wrap = on;
    }

    /// The text view's width may have settled (a splitter let go, a window resized, the setting flipped): wrap again
    /// to the columns that fit, once, and only if they differ from the ones the rows are wrapped to. The exact final
    /// wrap: a wrap still in flight for another width is replaced.
    pub fn settle_wrap(&mut self, app: &AppWindow) {
        // A held splitter is the live wrap's (`live_wrap`); the release settles.
        if app.get_panel_splitting() {
            return;
        }
        self.wrap_to_width(app, false);
    }

    /// The text view's width is moving (a splitter dragged, a window edge dragged): re-wrap to the columns that fit
    /// NOW, at every step, keeping the line at the top in place (2026-10-23, owner: "make that dynamic so it wraps
    /// as you move it"). A small file is wrapped where the step lands (a millisecond or two); a big one on a worker,
    /// one wrap in flight at a time, whose answer wraps again to the newest width, so a drag is never stalled.
    pub fn live_wrap(&mut self, app: &AppWindow) {
        // A sidebar slide wraps once, at its end (rule 217): no worker answer starts another wrap mid-slide.
        if app.get_sidebar_sliding() {
            return;
        }
        self.wrap_to_width(app, true);
    }

    fn wrap_to_width(&mut self, app: &AppWindow, live: bool) {
        enum Act {
            Nothing,
            Lines,
            Wrap(String, u64, Arc<Vec<SharedString>>, bool),
        }
        // Never while the slot is off show (Settings, the start screen, a window too narrow): what is there stays,
        // and the width that comes back settles it.
        if !app.get_panel_shown() {
            return;
        }
        let want = self.want_cols(app);
        let pending = self.wrap_pending;
        let act = match self.shown.as_ref() {
            // A big file's first wrap is on its way; its answer settles again.
            Some(Shown {
                content:
                    Content::Text {
                        lines,
                        wrapped_cols,
                        shaped,
                        ready: true,
                        ..
                    },
                tab,
                generation,
                ..
            }) => {
                if want == 0 {
                    if *wrapped_cols == 0 && pending == 0 {
                        Act::Nothing
                    } else {
                        Act::Lines
                    }
                } else if (want == *wrapped_cols && pending == 0) || want == pending {
                    Act::Nothing
                } else if live && pending != 0 {
                    // One wrap in flight: its answer wraps again to the width then.
                    Act::Nothing
                } else {
                    let worker = if live {
                        *shaped > WRAP_LIVE_INLINE_MAX
                    } else {
                        is_big(lines)
                    };
                    Act::Wrap(tab.clone(), *generation, Arc::clone(lines), worker)
                }
            }
            _ => Act::Nothing,
        };
        match act {
            Act::Nothing => {}
            Act::Lines => {
                self.next_wrap_seq();
                let top = self.top_line(app);
                if let Some(Shown {
                    content:
                        Content::Text {
                            lines,
                            shown,
                            nums,
                            wrapped_cols,
                            ..
                        },
                    ..
                }) = self.shown.as_mut()
                {
                    *shown = line_model(lines);
                    *nums = no_nums();
                    *wrapped_cols = 0;
                }
                self.push(app);
                self.restore_top(app, top);
            }
            Act::Wrap(tab, generation, lines, worker) => {
                if worker {
                    // The rows on show stay until the new ones are made.
                    self.spawn_wrap(&tab, generation, lines, want);
                    return;
                }
                self.next_wrap_seq();
                self.wraps += 1;
                let (rows, new_nums) = wrap_lines(&lines, want, &|| false);
                let top = self.top_line(app);
                if let Some(Shown {
                    content:
                        Content::Text {
                            shown,
                            nums,
                            wrapped_cols,
                            ..
                        },
                    ..
                }) = self.shown.as_mut()
                {
                    *shown = ModelRc::from(Rc::new(VecModel::from(rows)));
                    *nums = ModelRc::from(Rc::new(VecModel::from(new_nums)));
                    *wrapped_cols = want;
                }
                self.push(app);
                self.restore_top(app, top);
            }
        }
    }

    /// Pushes what is on show into the window's properties.
    fn push(&mut self, app: &AppWindow) {
        let Some(s) = &self.shown else {
            return;
        };
        let title = self
            .titles
            .get(&s.tab)
            .map_or_else(|| file_name(&s.target), String::as_str);
        app.set_panel_title(title.into());
        app.set_panel_actual(false);
        let empty_rows: ModelRc<SharedString> = ModelRc::default();
        match &s.content {
            Content::Reading => {
                app.set_panel_kind(0);
                app.set_panel_sub("".into());
                app.set_panel_note("Opening\u{2026}".into());
                app.set_panel_picture(Image::default());
                app.set_panel_alpha(false);
                app.set_panel_lines(empty_rows);
                app.set_panel_max_cols(0);
                app.set_panel_foot("".into());
                reset_wrap_props(app);
            }
            Content::Failed(why) => {
                app.set_panel_kind(0);
                app.set_panel_sub("".into());
                app.set_panel_note(why.as_str().into());
                app.set_panel_picture(Image::default());
                app.set_panel_alpha(false);
                app.set_panel_lines(empty_rows);
                app.set_panel_max_cols(0);
                app.set_panel_foot("".into());
                reset_wrap_props(app);
            }
            Content::Image {
                image,
                w,
                h,
                alpha,
                scaled,
            } => {
                let checker = self.checker.get_or_insert_with(checker_tile).clone();
                app.set_panel_kind(1);
                app.set_panel_sub(
                    format!("{w} x {h}{}", if *scaled { ", scaled down" } else { "" }).into(),
                );
                app.set_panel_note("".into());
                app.set_panel_picture(image.clone());
                app.set_panel_checker(checker);
                app.set_panel_alpha(*alpha);
                app.set_panel_lines(empty_rows);
                app.set_panel_max_cols(0);
                app.set_panel_foot("".into());
                reset_wrap_props(app);
            }
            // A big file waiting for its first wrap: the same as reading.
            Content::Text { ready: false, .. } => {
                app.set_panel_kind(0);
                app.set_panel_sub("".into());
                app.set_panel_note("Opening\u{2026}".into());
                app.set_panel_picture(Image::default());
                app.set_panel_alpha(false);
                app.set_panel_lines(empty_rows);
                app.set_panel_max_cols(0);
                app.set_panel_foot("".into());
                reset_wrap_props(app);
            }
            Content::Text {
                shown,
                nums,
                count,
                cols,
                foot,
                bytes,
                wrapped_cols,
                ..
            } => {
                app.set_panel_kind(2);
                let n = if *count == 1 {
                    "1 line".to_string()
                } else {
                    format!("{count} lines")
                };
                app.set_panel_sub(
                    format!(
                        "{n}, {}",
                        human_bytes(usize::try_from(*bytes).unwrap_or(usize::MAX))
                    )
                    .into(),
                );
                app.set_panel_note(
                    if *count == 0 {
                        "The file is empty."
                    } else {
                        ""
                    }
                    .into(),
                );
                app.set_panel_picture(Image::default());
                app.set_panel_alpha(false);
                app.set_panel_lines(shown.clone());
                app.set_panel_line_nums(nums.clone());
                app.set_panel_wrapped(*wrapped_cols > 0);
                app.set_panel_wrapped_cols(i32::try_from(*wrapped_cols).unwrap_or(0));
                app.set_panel_number_count(i32::try_from(*count).unwrap_or(i32::MAX));
                app.set_panel_max_cols(i32::try_from(*cols).unwrap_or(i32::MAX));
                app.set_panel_foot(foot.as_str().into());
            }
        }
        if let Some(why) = self.refused.get(&s.tab)
            && matches!(
                s.content,
                Content::Image { .. } | Content::Text { ready: true, .. }
            )
        {
            app.set_panel_sub(why.as_str().into());
        }
    }

    /// Nothing on show: the picture and the rows are let go.
    fn clear(&self, app: &AppWindow) {
        app.set_panel_kind(0);
        app.set_panel_title("".into());
        app.set_panel_sub("".into());
        app.set_panel_note("".into());
        app.set_panel_picture(Image::default());
        app.set_panel_alpha(false);
        app.set_panel_actual(false);
        app.set_panel_lines(ModelRc::default());
        app.set_panel_max_cols(0);
        app.set_panel_foot("".into());
        reset_wrap_props(app);
    }

    /// Reads the file on a worker thread; the answer is a `UiMsg::Panel`.
    fn spawn_load(&self, tab: &str, kind: &PanelKind, target: &str, generation: u64) {
        let post = Arc::clone(&self.post);
        let tab = tab.to_string();
        let target = target.to_string();
        let kind = kind.clone();
        let current = Arc::clone(&self.current);
        // Text that was handed over is shaped on the worker too (a 4 MB string is 100 000 rows).
        let inline = self
            .inline
            .get(tab.as_str())
            .filter(|i| i.target == target)
            .map(|i| Arc::clone(&i.text));
        let is_inline = target.starts_with(INLINE);
        let job = move || {
            let stale = || current.load(Ordering::SeqCst) != generation;
            if is_inline {
                let result = match inline {
                    Some(text) => Ok(TextRows::from_doc(preview::text_doc_from_str(
                        &text, TEXT_CAP,
                    ))),
                    None => Err(Reason::Other("the text is gone".to_string())),
                };
                if !stale() {
                    post(UiMsg::Panel(PanelMsgBox(PanelMsg::Text {
                        generation,
                        tab,
                        result,
                    })));
                }
                return;
            }
            // Every load, a click's or one restored from panels.json, is vetted the same way.
            let result = vetted(&target, &kind).and_then(|real| match kind {
                PanelKind::Image => {
                    // A poisoned lock only means an earlier decode panicked; the guard still guards.
                    let _one = DECODE
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if stale() {
                        return Err(Reason::Superseded);
                    }
                    preview::decode_image(Path::new(&real), &stale)
                        .map(|d| Loaded::Image(into_loaded(d)))
                }
                _ => preview::read_text(Path::new(&real), TEXT_CAP)
                    .map(|d| Loaded::Text(TextRows::from_doc(d))),
            });
            if stale() {
                return;
            }
            let msg = match result {
                Ok(Loaded::Image(l)) => PanelMsg::Image {
                    generation,
                    tab,
                    result: Ok(l),
                },
                Ok(Loaded::Text(t)) => PanelMsg::Text {
                    generation,
                    tab,
                    result: Ok(t),
                },
                Err(r) if kind == PanelKind::Image => PanelMsg::Image {
                    generation,
                    tab,
                    result: Err(r),
                },
                Err(r) => PanelMsg::Text {
                    generation,
                    tab,
                    result: Err(r),
                },
            };
            post(UiMsg::Panel(PanelMsgBox(msg)));
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-panel".into())
            .spawn(job)
        {
            log::warn!("could not start the panel reader: {e}");
        }
    }
}

/// The lines as the window's model (the strings are shared, not copied).
fn line_model(lines: &Arc<Vec<SharedString>>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(lines.as_ref().clone())))
}

/// No line numbers of its own: the rows are the lines and the gutter counts them.
fn no_nums() -> ModelRc<i32> {
    ModelRc::from(Rc::new(VecModel::<i32>::default()))
}

/// The lines' bytes as shaped (one newline each).
fn shaped_len(lines: &[SharedString]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

/// A file too big to wrap where it arrives.
fn is_big(lines: &[SharedString]) -> bool {
    shaped_len(lines) > WRAP_INLINE_MAX
}

/// The window shows no wrapped text: its numbers and flags go back to the default.
fn reset_wrap_props(app: &AppWindow) {
    app.set_panel_line_nums(no_nums());
    app.set_panel_wrapped(false);
    app.set_panel_wrapped_cols(0);
    app.set_panel_number_count(0);
}

/// The rows the lines wrap to at `cols`, and each row's line number (0 on a continuation). A line that fits is its
/// own row (the string is shared, not copied). Stops early when `stale` says so; the caller then drops the result.
pub(crate) fn wrap_lines(
    lines: &[SharedString],
    cols: usize,
    stale: &dyn Fn() -> bool,
) -> (Vec<SharedString>, Vec<i32>) {
    let cols = cols.max(flare_surfaces::wrap::MIN_COLS);
    let mut rows = Vec::with_capacity(lines.len());
    let mut nums = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i % 4096 == 0 && stale() {
            break;
        }
        let number = i32::try_from(i + 1).unwrap_or(i32::MAX);
        let parts = flare_surfaces::wrap::wrap_line(line.as_str(), cols);
        if parts.len() == 1 {
            rows.push(line.clone());
            nums.push(number);
            continue;
        }
        for (k, part) in parts.into_iter().enumerate() {
            rows.push(SharedString::from(part));
            nums.push(if k == 0 { number } else { 0 });
        }
    }
    (rows, nums)
}

enum Loaded {
    Image(LoadedImage),
    Text(TextRows),
}

/// What a load re-checks before it reads, whether the target came from a click or from the file
/// anyone can edit: the path as the file system names it (a mapped drive becomes a network name, a
/// link its target) must be on the click allowlist and of the same kind. Gives the path to read.
fn vetted(target: &str, kind: &PanelKind) -> Result<String, Reason> {
    let no = || Reason::Other("not allowed".to_string());
    // sync-ok: runs on a worker thread only
    if !Path::new(target).exists() {
        return Err(Reason::Missing);
    }
    let dir = Path::new(target)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (abs, real) =
        flare_term::path_open::resolve_openable_file(&dir, target, "").ok_or_else(no)?;
    (preview::classify_resolved(&abs, &real).as_ref() == Some(kind)
        && preview::loadable(&real, kind))
    .then_some(real)
    .ok_or_else(no)
}

fn into_loaded(d: Decoded) -> LoadedImage {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&d.rgba, d.width, d.height);
    LoadedImage {
        buf,
        has_alpha: d.has_alpha,
        scaled: d.scaled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_plain_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1 KB");
        assert_eq!(human_bytes(1025), "2 KB");
        assert_eq!(human_bytes(4 * 1024 * 1024), "4.0 MB");
        assert_eq!(human_bytes(1536 * 1024), "1.5 MB");
    }

    #[test]
    fn a_wide_line_scrolls_by_its_screen_columns_not_its_characters() {
        let doc = preview::TextDoc {
            lines: vec!["abc".into(), "\u{65e5}\u{672c}\u{8a9e}".repeat(50)],
            truncated: false,
            cut: None,
            bytes: 0,
        };
        // 150 characters, two columns each.
        assert_eq!(TextRows::from_doc(doc).cols, 300);
    }

    #[test]
    fn the_header_shows_the_file_name() {
        assert_eq!(file_name(r"C:\a\b\pic.png"), "pic.png");
        assert_eq!(file_name("C:/a/b/pic.png"), "pic.png");
        assert_eq!(file_name("pic.png"), "pic.png");
    }

    #[test]
    fn the_checker_tile_has_two_greys_in_a_two_by_two_pattern() {
        let img = checker_tile();
        let buf = img.to_rgba8().unwrap();
        let px = buf.as_slice();
        let at = |x: usize, y: usize| px[y * 16 + x].r;
        assert_eq!(at(0, 0), 0x9a);
        assert_eq!(at(8, 0), 0x7a);
        assert_eq!(at(0, 8), 0x7a);
        assert_eq!(at(8, 8), 0x9a);
        assert_eq!(at(7, 7), 0x9a);
    }
}
