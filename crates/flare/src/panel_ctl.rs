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

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use flare_surfaces::PanelKind;
use flare_surfaces::preview::{self, Cut, Decoded, LINES_MAX, Reason, TEXT_CAP};
use flare_surfaces::state::TabPanels;
use flare_surfaces::store::{PanelStore, SavedTabPanel, is_blank, match_restored};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

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
}

/// A text file ready for the window: the rows and the numbers are made on the worker, so the UI
/// thread only wraps them.
pub struct TextRows {
    pub rows: Vec<SharedString>,
    /// The widest row, in characters.
    pub cols: usize,
    /// The file's real length.
    pub bytes: u64,
    /// The foot's words when a cap cut the file short, else empty.
    pub foot: String,
}

impl TextRows {
    pub fn from_doc(doc: preview::TextDoc) -> Self {
        let cols = doc
            .lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let foot = match doc.cut {
            Some(Cut::Bytes) => format!("Only the first {} are shown.", human_bytes(TEXT_CAP)),
            Some(Cut::Lines) => format!("Only the first {LINES_MAX} lines are shown."),
            None => String::new(),
        };
        Self {
            rows: doc.lines.into_iter().map(SharedString::from).collect(),
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
        lines: ModelRc<SharedString>,
        count: usize,
        cols: usize,
        foot: String,
        bytes: u64,
    },
}

struct Shown {
    tab: String,
    kind: PanelKind,
    target: String,
    generation: u64,
    content: Content,
}

pub struct PanelCtl {
    panels: TabPanels,
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
                && (!p.kind.is_built() || preview::loadable(&p.target, &p.kind))
            {
                self.panels.restore(id, p);
            }
        }
        // What was read is what is on disk: the first persist writes only a real change.
        self.saved = None;
    }

    /// Shows `kind` / `target` on the tab and opens it. False for a target the panel will not load.
    pub fn show(&mut self, tab: &str, kind: PanelKind, target: &str) -> bool {
        if kind.is_built() && !preview::loadable(target, &kind) {
            return false;
        }
        // The file may have changed since it was read, or failed and now exist: a click on what
        // is on show reads it again.
        if self.shown.as_ref().is_some_and(|s| s.tab == tab) {
            self.reload = true;
        }
        self.panels.show(tab, kind, target)
    }

    pub fn close(&mut self, tab: &str) -> bool {
        self.panels.close(tab)
    }

    pub fn reopen(&mut self, tab: &str) -> bool {
        self.panels.reopen(tab)
    }

    pub fn is_open(&self, tab: &str) -> bool {
        self.panels.open_on(tab).is_some()
    }

    /// The tab has a panel (open or closed) of a kind this build draws.
    pub fn has_built(&self, tab: &str) -> bool {
        self.panels.get(tab).is_some_and(|p| p.kind.is_built())
    }

    pub fn forget(&mut self, tab: &str) {
        self.panels.forget(tab);
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
                panel: self.panels.get(id).cloned(),
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
        let want = front.and_then(|t| {
            self.panels
                .open_on(t)
                .filter(|p| p.kind.is_built())
                .map(|p| (t.to_string(), p.kind.clone(), p.target.clone(), p.width))
        });
        app.set_panel_open(want.is_some());
        app.set_panel_share(want.as_ref().and_then(|w| w.3).unwrap_or(0.0));
        let Some((tab, kind, target, _)) = want else {
            if self.shown.take().is_some() {
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
            return;
        }
        self.reload = false;
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
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let (generation, tab) = match &msg {
            PanelMsg::Image {
                generation, tab, ..
            }
            | PanelMsg::Text {
                generation, tab, ..
            } => (*generation, tab.as_str()),
        };
        if generation != shown.generation || tab != shown.tab {
            return;
        }
        shown.content = match msg {
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
                Ok(t) => Content::Text {
                    count: t.rows.len(),
                    lines: ModelRc::from(Rc::new(VecModel::from(t.rows))),
                    cols: t.cols,
                    foot: t.foot,
                    bytes: t.bytes,
                },
                Err(r) => Content::Failed(r.to_string()),
            },
        };
        self.push(app);
    }

    /// Pushes what is on show into the window's properties.
    fn push(&mut self, app: &AppWindow) {
        let Some(s) = &self.shown else {
            return;
        };
        app.set_panel_title(file_name(&s.target).into());
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
            }
            Content::Text {
                lines,
                count,
                cols,
                foot,
                bytes,
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
                app.set_panel_lines(lines.clone());
                app.set_panel_max_cols(i32::try_from(*cols).unwrap_or(i32::MAX));
                app.set_panel_foot(foot.as_str().into());
            }
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
    }

    /// Reads the file on a worker thread; the answer is a `UiMsg::Panel`.
    fn spawn_load(&self, tab: &str, kind: &PanelKind, target: &str, generation: u64) {
        let post = Arc::clone(&self.post);
        let tab = tab.to_string();
        let target = target.to_string();
        let kind = kind.clone();
        let current = Arc::clone(&self.current);
        let job = move || {
            let stale = || current.load(Ordering::SeqCst) != generation;
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
