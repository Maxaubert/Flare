//! The panel launcher and the panel header's "Open in" in the window (2026-11-24, rules 1384 to 1388). The rules live
//! in `crate::launcher` (pure, with its tests); this file feeds them the pointer, the keys and the grace timer, pushes
//! what they say into the window, and does what a row or the button asks with the calls that already exist: the web
//! panel's `browser_open`, the device panel's `device_open`, the path a file link's "Show in side panel" takes
//! (`show_preview_asked`), `PanelCtl::reopen`, the path openers and `open_url`. The web bar's More menu (rule 1389)
//! lives here too: its "Open in browser" is the page's "Open in".

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use flare_term::keys::KeyEvent;
use flare_term::path_open::{OpenMode, open_term_path};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::{AppCtl, KeyOutcome, PickAnswer, TermMsgBox, UiMsg};
use crate::launcher::{self, Hover, Item, Offer, OpenIn, Where};
use crate::term_ui::TermMsg;
use crate::{AppWindow, LauncherRow};

/// The system's Open dialog: the folder it starts in, and where its answer goes (on a worker in production).
pub type PickFile = Rc<dyn Fn(Option<PathBuf>, PickAnswer)>;

/// The grace timer's name in the registry (`timer_registry`).
const GRACE_TIMER: &str = "panel.launcher-grace";

/// A worker's answer for the launcher. It prints its kind only, never a path or a name.
pub enum LauncherMsg {
    /// The file picked in "Open a file..." is not one the panel shows.
    Refused { tab: String },
    /// The default app of files of `ext`, as the system names it (None: none, or no name).
    AppName { ext: String, name: Option<String> },
}

impl std::fmt::Debug for LauncherMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LauncherMsg::Refused { .. } => "LauncherMsg::Refused",
            LauncherMsg::AppName { .. } => "LauncherMsg::AppName",
        })
    }
}

/// What the window's launcher and "Open in" hold between refreshes.
#[derive(Default)]
pub(crate) struct LauncherState {
    hover: Hover,
    /// The menu's rows now, and the ones last handed to the window (None: never).
    items: Vec<Item>,
    pushed: Option<Vec<Item>>,
    /// The lit row, -1 for none.
    hot: i32,
    /// The tab the button stands for (the shell tab in front while it shows).
    tab: Option<String>,
    shown: bool,
    pick_file: Option<PickFile>,
    /// Default apps' names by extension, once looked up (None inside: none to name), and the lookups on their way.
    app_names: HashMap<String, Option<String>>,
    asking: std::collections::HashSet<String>,
    /// The find bar was open at the last sync (it shares the corner, so the button follows it).
    find_open: bool,
}

impl AppCtl {
    /// The system's Open dialog for "Open a file..." (startup; tests hand a fake).
    pub fn set_pick_file(&mut self, pick: PickFile) {
        self.launcher.pick_file = Some(pick);
    }

    /// The default app's name for files of `ext`, as a worker would have found it (tests).
    #[cfg(test)]
    pub(crate) fn set_app_name_for_test(&mut self, ext: &str, name: Option<&str>) {
        self.launcher
            .app_names
            .insert(ext.to_string(), name.map(str::to_string));
    }

    // ---- what the window shows ----

    /// The launcher and "Open in" follow the tab in front (every refresh, after the panels).
    pub(super) fn sync_launcher(&mut self, app: &AppWindow) {
        let front = self
            .model
            .active_shell()
            .filter(|_| self.view.front_kind == 0);
        let tab = front.map(|t| t.id.clone());
        let remote = front.is_some_and(|t| t.remote.is_some());
        let shown = launcher::shown(Where {
            shell_front: tab.is_some(),
            // What the window shows (an open panel of a kind this build draws), as `PanelCtl::sync` set it: a stored
            // panel this machine cannot draw (rule 1316) leaves the way in.
            panel_open: tab.is_some() && app.get_panel_open(),
            panel_fits: app.get_panel_fits(),
            remote_view: self.remote_convo_shown(),
            skeleton: app.get_skeleton_shown(),
            find_open: app.get_find_open(),
        });
        self.launcher.find_open = app.get_find_open();
        // Another tab in front, or the button gone (the panel opened, the window too narrow): the menu goes with it.
        if !shown || tab != self.launcher.tab {
            self.launcher_close();
        }
        self.launcher.tab = tab.clone();
        self.launcher.shown = shown;
        let snap = tab.as_deref().and_then(|t| self.panels.snap(t));
        self.launcher.items = launcher::items(Offer {
            device: self.device.offered(),
            file: !remote,
            // Only a kind this build draws comes back (as for Ctrl+Shift+P).
            last: tab.as_deref().is_some_and(|t| self.panels.has_built(t))
                && launcher::last_ok(snap.as_ref()),
        });
        if self.launcher.hot >= self.launcher.items.len() as i32 {
            self.launcher.hot = -1;
        }
        if self.launcher.pushed.as_ref() != Some(&self.launcher.items) {
            let tops = launcher::tops(&self.launcher.items);
            let rows: Vec<LauncherRow> = self
                .launcher
                .items
                .iter()
                .zip(tops)
                .map(|(i, (top, _))| LauncherRow {
                    label: i.label().into(),
                    icon: i.icon().into(),
                    top,
                    rule: launcher::ruled(*i),
                })
                .collect();
            app.set_launcher_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
            app.set_launcher_menu_h(launcher::menu_h(&self.launcher.items));
            self.launcher.pushed = Some(self.launcher.items.clone());
        }
        app.set_launcher_shown(shown);
        self.push_launcher(app);
        self.sync_open_in(app, snap.as_ref());
    }

    /// The find bar opened or closed outside a refresh (`push_term`): the launcher follows it at once.
    pub(super) fn launcher_find_changed(&mut self, app: &AppWindow, open: bool) {
        if open != self.launcher.find_open {
            self.sync_launcher(app);
        }
    }

    /// The menu's open state and lit row.
    fn push_launcher(&self, app: &AppWindow) {
        app.set_launcher_open(self.launcher.shown && self.launcher.hover.is_open());
        app.set_launcher_hot(self.launcher.hot);
    }

    fn push_launcher_now(&self) {
        if let Some(app) = self.app() {
            self.push_launcher(&app);
            app.window().request_redraw();
        }
    }

    /// The panel header's "Open in" words for the front tab's open panel ("" hides the button).
    fn sync_open_in(&mut self, app: &AppWindow, snap: Option<&crate::panel_ctl::PanelSnap>) {
        let Some(o) = launcher::open_in(snap) else {
            app.set_panel_open_in("".into());
            app.set_panel_open_in_short("".into());
            return;
        };
        let mac = cfg!(target_os = "macos");
        let app_name = match &o {
            OpenIn::File {
                path,
                reveal: false,
            } => self.app_name_for(path),
            _ => None,
        };
        app.set_panel_open_in(launcher::open_in_label(&o, app_name.as_deref(), mac, false).into());
        app.set_panel_open_in_short(launcher::open_in_label(&o, None, mac, true).into());
    }

    /// The default app's name for this file's type when it is known; else None, and a worker looks it up once per
    /// type (the registry is never read on the UI thread, rule 34). Under `--e2e` and in tests nothing is looked up,
    /// so the words are the same on every machine.
    fn app_name_for(&mut self, path: &str) -> Option<String> {
        let ext = launcher::ext_key(path)?;
        if let Some(known) = self.launcher.app_names.get(&ext) {
            return known.clone();
        }
        if self.deps.e2e || !self.launcher.asking.insert(ext.clone()) {
            return None;
        }
        let post = Arc::clone(&self.deps.post);
        let spawned = std::thread::Builder::new()
            .name("flare-app-name".into())
            .spawn(move || {
                let name = flare_win::openers::default_app_name(&ext)
                    .and_then(|n| launcher::clean_app_name(&n));
                post(UiMsg::Launcher(LauncherMsg::AppName { ext, name }));
            });
        if let Err(e) = spawned {
            log::warn!("could not start the app name lookup: {e}");
        }
        None
    }

    pub(super) fn launcher_msg(&mut self, m: LauncherMsg) {
        match m {
            LauncherMsg::Refused { tab } => self.say_in_panel(
                &tab,
                "Flare cannot show this kind of file in the side panel.",
                "Cannot show that file here",
                "Cannot show this file",
            ),
            LauncherMsg::AppName { ext, name } => {
                self.launcher.asking.remove(&ext);
                self.launcher.app_names.insert(ext, name);
                self.refresh();
            }
        }
    }

    // ---- the pointer and the grace ----

    /// The pointer came onto (`on`) or left the button (zone 0) or the menu with its bridge (zone 1).
    pub(super) fn launcher_hover(&mut self, zone: i32, on: bool) {
        // A hidden button opens nothing, but the pointer leaving it is still heard, so a button that shows again does
        // not think the pointer is on it.
        if !self.launcher.shown && on {
            return;
        }
        let now = self.now();
        match zone {
            0 => {
                if self.launcher.hover.button(on, now) {
                    self.launcher.hot = -1;
                }
            }
            _ => {
                self.launcher.hover.menu(on, now);
                // Off the menu the pointer lights nothing; a row the keys picked stays lit.
                if !on && !self.launcher.hover.keyed() {
                    self.launcher.hot = -1;
                }
            }
        }
        self.arm_grace(now);
        self.push_launcher_now();
    }

    /// A click on the button (or its accessible action): see `Hover::click`. A menu opened this way lights its first
    /// row, so Enter picks it.
    pub(super) fn launcher_clicked(&mut self) {
        if !self.launcher.shown {
            return;
        }
        let was = self.launcher.hover.is_open();
        self.launcher.hover.click();
        if self.launcher.hover.keyed() && (!was || self.launcher.hot < 0) {
            self.launcher.hot = 0;
        }
        if !self.launcher.hover.is_open() {
            self.launcher.hot = -1;
            crate::timer_registry::clear(GRACE_TIMER);
        }
        self.push_launcher_now();
    }

    /// The pointer moved over the menu: the row under it is lit (`y` from the menu's top edge).
    pub(super) fn launcher_menu_moved(&mut self, y: f32) {
        if !self.launcher.hover.is_open() {
            return;
        }
        let hot = launcher::row_at(&self.launcher.items, y).map_or(-1, |i| i as i32);
        if hot != self.launcher.hot {
            self.launcher.hot = hot;
            self.push_launcher_now();
        }
    }

    /// A click in the menu: the row under it is picked (the padding, a rule and the bridge pick nothing).
    pub(super) fn launcher_menu_clicked(&mut self, y: f32) {
        if !self.launcher.hover.is_open() {
            return;
        }
        if let Some(i) = launcher::row_at(&self.launcher.items, y) {
            self.launcher_pick(i as i32);
        }
    }

    /// Starts (or stops) the grace timer for when the menu closes.
    fn arm_grace(&mut self, now: u64) {
        match self.launcher.hover.due() {
            Some(at) => {
                let weak = self.me.clone();
                crate::timer_registry::set(
                    GRACE_TIMER,
                    at.saturating_sub(now).max(1),
                    false,
                    move || {
                        if let Some(rc) = weak.upgrade()
                            && let Ok(mut c) = rc.try_borrow_mut()
                        {
                            c.launcher_grace();
                        }
                    },
                );
            }
            None => crate::timer_registry::clear(GRACE_TIMER),
        }
    }

    /// The grace timer fired: the menu closes when the grace has run out with the pointer on neither part, else the
    /// timer waits for what is left of it.
    fn launcher_grace(&mut self) {
        let now = self.now();
        if self.launcher.hover.tick(now) {
            self.launcher.hot = -1;
            self.push_launcher_now();
        } else {
            self.arm_grace(now);
        }
    }

    fn launcher_close(&mut self) {
        self.launcher.hover.close();
        self.launcher.hot = -1;
        crate::timer_registry::clear(GRACE_TIMER);
    }

    // ---- the keys ----

    /// A key while the menu is open. A menu a click or the keys opened holds the arrows, Home, End, Enter and Escape;
    /// any other key closes it and goes where it would have gone. A menu the pointer opened takes no key: Escape closes
    /// it and still reaches the shell (an agent hears its Escape). None: the key goes on.
    pub(super) fn launcher_key(&mut self, ev: &KeyEvent) -> Option<KeyOutcome> {
        if !ev.pressed || !self.launcher.hover.is_open() {
            return None;
        }
        let keyed = self.launcher.hover.keyed();
        let len = self.launcher.items.len();
        let hot = self.launcher.hot;
        let to = match ev.key.as_str() {
            "Escape" => {
                self.launcher_close();
                self.push_launcher_now();
                return keyed.then_some(KeyOutcome::Consumed);
            }
            _ if !keyed => return None,
            "ArrowDown" => launcher::step(len, hot, 1),
            "ArrowUp" => launcher::step(len, hot, -1),
            "Home" => launcher::step(len, -1, 1),
            "End" => launcher::step(len, -1, -1),
            "Enter" | "NumpadEnter" => {
                if hot >= 0 {
                    self.launcher_pick(hot);
                }
                return Some(KeyOutcome::Consumed);
            }
            // A modifier alone leaves the menu up.
            "Shift" | "Control" | "Alt" | "Meta" => return Some(KeyOutcome::Consumed),
            _ => {
                self.launcher_close();
                self.push_launcher_now();
                return None;
            }
        };
        self.launcher.hot = to;
        self.push_launcher_now();
        Some(KeyOutcome::Consumed)
    }

    // ---- the rows ----

    /// A row was picked (a click, Enter, or its accessible action): the menu closes and the row does what it says.
    pub(super) fn launcher_pick(&mut self, index: i32) {
        let Some(item) = usize::try_from(index)
            .ok()
            .and_then(|i| self.launcher.items.get(i).copied())
        else {
            return;
        };
        let Some(tab) = self.launcher.tab.clone() else {
            return;
        };
        self.launcher_close();
        self.push_launcher_now();
        match item {
            // The tab's web panel in its new-tab state: an empty page, the address field holding the keys.
            Item::Browser => match self.browser_open(&tab, "about:blank") {
                Ok(_) => self.focus_address(),
                Err(e) => log::info!("the launcher's browser did not open: {}", e.message),
            },
            // The device panel on its list (a device the tab showed before comes back through Last panel).
            Item::Device => {
                self.device_open(&tab, None);
            }
            Item::File => self.launcher_pick_file(&tab),
            Item::Last => {
                if self.panels.reopen(&tab) {
                    self.refresh();
                    self.panel_changed(&tab);
                }
            }
        }
        self.refresh();
    }

    /// "Open a file...": the system's Open dialog, starting in the tab's folder. The file picked goes the way a file
    /// link's "Show in side panel" goes: a worker resolves and vets it again (rules 13, 14) and the panel shows it as
    /// what it is; a file the panel cannot show is said in the panel. A cancel does nothing.
    fn launcher_pick_file(&mut self, tab: &str) {
        let Some(pick) = self.launcher.pick_file.clone() else {
            log::info!("no file picker in this run");
            return;
        };
        let dir = self
            .model
            .state
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .map(|t| t.cwd.clone())
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        let post = Arc::clone(&self.deps.post);
        let home = self.deps.home.clone();
        let tab = tab.to_string();
        let answer: PickAnswer = Arc::new(move |picked: Option<PathBuf>| {
            let Some(p) = picked else { return };
            let (post, home, tab) = (Arc::clone(&post), home.clone(), tab.clone());
            // The dialog answers on its own worker in production; the vetting reads the disk, so it gets a worker of
            // its own either way (a test's picker answers on the UI thread).
            let spawned = std::thread::Builder::new()
                .name("flare-launcher-file".into())
                .spawn(move || {
                    let base = p.parent().map(|d| d.to_string_lossy().into_owned());
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                    let shown = base
                        .zip(name)
                        .and_then(|(b, n)| crate::term_ui::previewable(&b, &n, &home));
                    match shown {
                        Some((kind, abs)) => post(UiMsg::Term(TermMsgBox(TermMsg::Preview {
                            tab,
                            kind,
                            abs,
                            line: None,
                            asked: true,
                        }))),
                        None => post(UiMsg::Launcher(LauncherMsg::Refused { tab })),
                    }
                });
            if let Err(e) = spawned {
                log::warn!("could not start the picked file's check: {e}");
            }
        });
        pick(dir, answer);
    }

    // ---- Open in ----

    /// The panel header's "Open in": the front tab's page in the default browser, or its file in its default app (a
    /// file a click would never open is shown in its folder instead, rule 14). On a worker: the process start can
    /// wait on a slow disk.
    pub(super) fn panel_open_in(&mut self) {
        let Some(tab) = self.front_id() else { return };
        let snap = self.panels.snap(&tab);
        match launcher::open_in(snap.as_ref()) {
            Some(OpenIn::Url(url)) => (self.deps.open_url)(&url),
            Some(OpenIn::File { path, reveal }) => {
                let openers = Arc::clone(&self.deps.term.openers);
                let home = self.deps.home.clone();
                let p = std::path::Path::new(&path);
                let base = p.parent().map(|d| d.to_string_lossy().into_owned());
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                let Some((base, name)) = base.zip(name) else {
                    return;
                };
                let mode = if reveal {
                    OpenMode::Reveal
                } else {
                    OpenMode::Open
                };
                let spawned = std::thread::Builder::new()
                    .name("flare-open-in".into())
                    .spawn(move || {
                        let _ = open_term_path(&base, &name, mode, &*openers, &home);
                    });
                if let Err(e) = spawned {
                    log::warn!("could not start Open in: {e}");
                }
            }
            None => {}
        }
    }

    // ---- the web bar's More menu (rule 1389) ----

    /// The rows the web bar's More menu has for the tab's page now: "Open in browser" for a web address (the rules of
    /// "Open in", rule 1388), "Draw arrow" while it is offered (rule 256, with the window's room rule).
    fn web_more_rows(
        &self,
        app: &AppWindow,
        tab: &str,
    ) -> Vec<(crate::menus_ui::MenuRowData, String)> {
        let snap = self.panels.snap(tab);
        let open_in = matches!(launcher::open_in(snap.as_ref()), Some(OpenIn::Url(_)));
        let draw = (self.draw_arrow_offered(tab) && app.get_panel_draw_visible())
            .then(|| self.web.drawing(tab));
        crate::menus_ui::web_more_rows(open_in, draw)
            .into_iter()
            .map(|(r, action)| (r, action.to_string()))
            .collect()
    }

    /// A click on the web bar's More button: the window's shared menu drops from it (its right edge on the button's,
    /// under its foot first), with the rows the page has now. With no row there is nothing to show.
    pub(super) fn web_more_clicked(&mut self, right: f32, top: f32, foot: f32) {
        let Some(tab) = self.front_id() else { return };
        let Some(app) = self.app() else { return };
        let rows = self.web_more_rows(&app, &tab);
        if rows.is_empty() {
            return;
        }
        self.close_overflow_quietly();
        self.show_menu_from(
            crate::menus_ui::MenuKind::WebMore { tab },
            rows,
            right,
            top,
            Some(foot),
            true,
        );
    }

    /// A row of the web bar's More menu was picked (the menu is already closed). The tab must still be in front.
    pub(super) fn web_more_picked(&mut self, tab: &str, action: &str) {
        use crate::menus_ui::{WEB_MORE_DRAW, WEB_MORE_OPEN, WEB_MORE_STOP_DRAW};
        if self.front_id().as_deref() != Some(tab) {
            return;
        }
        match action {
            // Only a web address goes out, as from "Open in" (rule 1388).
            WEB_MORE_OPEN => {
                if let Some(OpenIn::Url(url)) = launcher::open_in(self.panels.snap(tab).as_ref()) {
                    (self.deps.open_url)(&url);
                }
            }
            // The page shows live again first (the menu over it made it a still, which cannot be drawn on), then the
            // draw mode starts as from the old button. A mode already on stays on.
            WEB_MORE_DRAW => {
                self.refresh();
                if !self.web.drawing(tab) {
                    self.panel_draw_arrow();
                }
            }
            // The menu over the page may have ended the mode already (rule 256): a stop never starts it again.
            WEB_MORE_STOP_DRAW if self.web.drawing(tab) => {
                self.web.toggle_draw(tab);
            }
            _ => {}
        }
    }
}
