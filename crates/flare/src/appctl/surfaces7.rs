//! Live preview, the agent board and question notifications, in the window (Surfaces stage 7).
//!
//! The rules are `flare_surfaces::{devpreview, board, notify}` (pure, with their tests) and
//! `flare_term::dev_server` (what a program printed). This file is the part that needs `AppCtl`'s state: the tabs, the
//! hub, the panels, the focus and the timer.
//!
//! - **Preview chip.** The terminal reads local server addresses off its own screen, in any tab. After a tab prints,
//!   the window asks its handle what it read (the terminal looks at its screen at most every 40 ms); the chip waits for a
//!   listener on that port (a worker looks in the system's listening table, never connecting to it), and goes when
//!   nothing listens or the screen was cleared.
//! - **Board.** A native panel kind. Its rows are made from the hub on the UI thread whenever the hub says something
//!   changed, and once a second while the board is open for the clocks. A closed board has no rows and no timer.
//! - **Notifications.** On each look at the hub the notifier decides which questions get a toast; a toast goes to the
//!   sink the window installed (Windows' own, or under `--e2e` a recording one).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use flare_settings::agent_prefs::notify_ask_on;
use flare_settings::term_prefs::dev_preview;
use flare_surfaces::PanelKind;
use flare_surfaces::board::{self, BoardInput, BoardRow, BoardState};
use flare_surfaces::devpreview::{ChipServer, DevChips};
use flare_surfaces::notify::{Asking, Look, Notifier, Toast};
use flare_term::dev_server::{self, LocalServer};
use slint::{ModelRc, VecModel};

use super::{AppCtl, Post, UiMsg};
use crate::menus_ui::MenuRowData;
use crate::{AppWindow, BoardRowUi};
use flare_tabs::keymap::ChordId;

/// The panel kind number of the agent board in the window (`panel-kind`).
pub const KIND_BOARD: i32 = 6;
/// How soon a screen the terminal has not read yet is asked about again.
pub const DEV_RETRY_MS: u64 = 50;
/// How often the open board's clocks move.
pub const BOARD_TICK_MS: u64 = 1000;
/// The rows the terminal's menu gains for the board.
pub const BOARD_SHOW: &str = "Show agent board";
pub const BOARD_HIDE: &str = "Hide agent board";

/// Looks whether anything listens on a local port. Production reads the system's listening table; the tests hand a
/// table of their own.
pub type PortProbe = Arc<dyn Fn(u16) -> bool + Send + Sync>;
/// Shows a notification. Production asks Windows; under `--e2e` the sink only records.
pub type ToastSink = Rc<dyn Fn(&Toast)>;

/// What the window holds for stage 7.
pub(super) struct Surf7 {
    pub dev: DevChips,
    pub probe: PortProbe,
    pub notifier: Notifier,
    pub toast: Option<ToastSink>,
    pub board: BoardState7,
    /// When the open board's clocks next move.
    pub board_due: Option<u64>,
    /// How many notifications were handed to the sink (the e2e and the tests read it).
    pub toasts: u64,
    /// The previews switch as last seen, so a change of it is noticed once.
    pub dev_on: bool,
    /// Tabs whose screen has output the terminal has not read yet (asked again too soon), and when to ask each
    /// again. One deadline per tab: another tab's output never pushes it back.
    pub dev_retry: std::collections::HashMap<String, u64>,
}

impl Surf7 {
    pub fn new() -> Self {
        Self {
            dev: DevChips::new(),
            probe: Arc::new(|_| false),
            notifier: Notifier::new(),
            toast: None,
            board: BoardState7::default(),
            board_due: None,
            toasts: 0,
            dev_on: true,
            dev_retry: std::collections::HashMap::new(),
        }
    }
}

/// What was last written into the window for the board.
#[derive(Default)]
pub(super) struct BoardState7 {
    pushed: Option<Pushed>,
    populated: bool,
}

#[derive(PartialEq)]
struct Pushed {
    rows: Vec<BoardRow>,
    current: Option<String>,
    inks: [slint::Color; 4],
}

/// The agent's name as a notification says it.
fn agent_name(k: Option<flare_agent::kinds::DetectedAgent>) -> &'static str {
    use flare_agent::kinds::DetectedAgent;
    match k {
        Some(DetectedAgent::Claude) => board::BoardAgent::Claude.name(),
        Some(DetectedAgent::Codex) => board::BoardAgent::Codex.name(),
        _ => board::BoardAgent::Other.name(),
    }
}

/// "localhost:5173" for the chip: the address without its scheme and path.
pub fn chip_label(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// A chip's address, as the web panel would take it. None for anything the panel would not.
fn chip_server(s: &LocalServer) -> Option<ChipServer> {
    let url = flare_surfaces::web::normalize_url(&s.url).ok()?;
    // The same rule again on the normal form: the chip only ever offers a local address.
    let local = dev_server::parse_local(&url)?;
    Some(ChipServer {
        url,
        port: local.port,
    })
}

impl AppCtl {
    /// How the window finds out whether a local port has a listener (startup; the tests hand a stand-in).
    pub fn set_port_probe(&mut self, probe: PortProbe) {
        self.s7.probe = probe;
    }

    /// Where notifications go (startup; the tests and `--e2e` hand a recorder).
    pub fn set_toast_sink(&mut self, sink: ToastSink) {
        self.s7.toast = Some(sink);
    }

    /// What the control channel would say of the tab's panel (the tests and the e2e read it).
    pub(crate) fn panel_snap(&self, tab: &str) -> Option<crate::panel_ctl::PanelSnap> {
        self.panels.snap(tab)
    }

    // ---- the Preview chip ------------------------------------------------------------------------------------------

    /// A tab printed: asks its terminal what local server address it read, and starts the look at the port when that is
    /// news. One lock on the emulator, which reads its own screen at most every 40 ms (see `Emulator::dev_state`); a
    /// comparison of counters here.
    pub(super) fn dev_read(&mut self, tab: &str) {
        if !dev_preview(&self.prefs) {
            return;
        }
        let Some(handle) = self.deps.host.handle(tab) else {
            return;
        };
        let st = handle.dev_server();
        // The terminal reads its screen at most every 40 ms; output it has not read yet is asked for again.
        let now = self.now();
        if handle.dev_pending() {
            // Keep an earlier deadline of this tab: its own steady output must not push it back either.
            self.s7
                .dev_retry
                .entry(tab.to_string())
                .or_insert(now + DEV_RETRY_MS);
        } else {
            self.s7.dev_retry.remove(tab);
        }
        let moved = match self.s7.dev.seen_seq(tab) {
            Some(seen) => seen != st.seq,
            None => st.seq != 0,
        };
        let server = st.server.as_ref().and_then(chip_server);
        if moved {
            self.s7.dev.on_reading(tab, st.seq, server.clone(), now);
        }
        // The same address printed again: acts only where no chip is live (a server started again).
        self.s7.dev.on_reprint(tab, st.reprint, server, now);
        self.dev_pump();
        self.sync_chip();
    }

    /// A tab closed or its shell ended: nothing of it is kept.
    pub(super) fn dev_forget(&mut self, tab: &str) {
        self.s7.dev.forget(tab);
        self.s7.dev_retry.remove(tab);
    }

    /// A setting changed: if it is the previews switch, the chips follow at once.
    pub(super) fn dev_settings_changed(&mut self) {
        let on = dev_preview(&self.prefs);
        if on != self.s7.dev_on {
            self.s7.dev_on = on;
            self.dev_resync_all();
        }
    }

    /// The setting changed: every tab is read once more (turning it on shows what is already listening), or every chip
    /// goes (turning it off).
    pub(super) fn dev_resync_all(&mut self) {
        if dev_preview(&self.prefs) {
            for id in self.deps.host.live_ids() {
                self.dev_read(&id);
            }
        } else {
            for id in self.deps.host.live_ids() {
                self.dev_forget(&id);
            }
        }
        self.sync_chip();
    }

    /// Starts the looks that are due, each on its own short-lived worker (rule 34); the answer is a `UiMsg::DevLook`.
    pub(super) fn dev_pump(&mut self) {
        let now = self.now();
        for check in self.s7.dev.due(now) {
            let probe = Arc::clone(&self.s7.probe);
            let post = Arc::clone(&self.deps.post);
            let job = move || {
                let up = probe(check.port);
                post(UiMsg::DevLook {
                    tab: check.tab,
                    token: check.token,
                    up,
                });
            };
            if let Err(e) = std::thread::Builder::new()
                .name("flare-port-look".into())
                .spawn(job)
            {
                log::warn!("could not start the port look: {e}");
            }
        }
        self.kick();
    }

    /// A worker's answer about a port.
    pub(super) fn on_dev_look(&mut self, tab: &str, token: u64, up: bool) {
        let now = self.now();
        self.s7.dev.on_check(tab, token, up, now);
        self.dev_pump();
        self.sync_chip();
    }

    /// The chip on the window: shown for the tab in front while it holds a listening local server and the panel does not
    /// show that page already.
    pub(super) fn sync_chip(&mut self) {
        let Some(app) = self.app() else { return };
        // Only the tab in front, and not while the window is down, has its chip looked at again.
        let front = self.model.active_shell().map(|t| t.id.clone());
        let in_view = if self.minimised {
            None
        } else {
            front.as_deref()
        };
        let now = self.now();
        if self.s7.dev.set_focus(in_view, now) {
            self.dev_pump();
        }
        let label = self
            .chip_for_front()
            .map_or_else(String::new, |c| chip_label(&c.url));
        if app.get_preview_chip_label().as_str() != label {
            app.set_preview_chip_label(label.into());
        }
    }

    /// The chip the tab in front offers, if any.
    fn chip_for_front(&self) -> Option<ChipServer> {
        if !dev_preview(&self.prefs) {
            return None;
        }
        let front = self.model.active_shell()?.id.clone();
        let chip = self.s7.dev.shown(&front)?.clone();
        // The panel already shows it: nothing to offer.
        if self.panels.web_target(&front).as_deref() == Some(chip.url.as_str()) {
            return None;
        }
        Some(chip)
    }

    /// The chip was clicked: the web panel opens on the tab at that address. The address is checked once more here, as
    /// the chip may only ever open a local one.
    pub(super) fn preview_chip_clicked(&mut self) {
        let Some(chip) = self.chip_for_front() else {
            return;
        };
        let Some(front) = self.front_id() else { return };
        if dev_server::parse_local(&chip.url).is_none() {
            return;
        }
        if self.browser_open(&front, &chip.url).is_ok() {
            self.sync_chip();
        }
    }

    // ---- the agent board -------------------------------------------------------------------------------------------

    /// Ctrl+Shift+A: shows the agent board in the front tab's panel, or closes it there. True when the key was used.
    pub(crate) fn toggle_board(&mut self) -> bool {
        let Some(id) = self.front_id() else {
            return false;
        };
        if !self.is_shell_tab(&id) {
            return false;
        }
        if self.panels.board_open_on(&id) {
            if self.panels.close(&id) {
                self.refresh();
                self.focus_terminal();
                self.panel_changed(&id);
            }
            return true;
        }
        self.open_convo_panel(&id, PanelKind::Board)
    }

    /// The rows of the board now: every tab with an agent, in the strip's order.
    pub(crate) fn board_rows(&self) -> Vec<BoardRow> {
        let inputs: Vec<BoardInput> = self
            .shell_tabs()
            .into_iter()
            .filter_map(|(tab, _)| {
                let e = self.hub.board_entry(&tab)?;
                let row = self.view.rows.iter().find(|r| r.id == tab);
                Some(BoardInput {
                    name: row.map_or_else(String::new, |r| r.label.clone()),
                    folder: row.map_or_else(String::new, |r| r.folder.clone()),
                    tab,
                    agent: e.agent,
                    state: e.state,
                    in_state_ms: e.in_state_ms,
                    question: e.question,
                    failed_kind: e.failed_kind,
                })
            })
            .collect();
        board::rows(&inputs)
    }

    /// The mark colour of a board state (the tab marks' own).
    fn board_ink(&self, s: BoardState) -> slint::Color {
        match s {
            BoardState::Working => self.marks.working,
            BoardState::Question => self.marks.question,
            BoardState::Finished => self.marks.finished,
            BoardState::Failed => self.marks.failed,
            BoardState::Idle => self.marks.ink,
        }
    }

    /// Writes the board into the window when the front tab's panel is the board; lets go of its rows otherwise. Also keeps
    /// the one-second clock of an open board.
    pub(super) fn sync_board(&mut self, app: &AppWindow) {
        let front = self.model.active_shell().map(|t| t.id.clone());
        let open = front
            .as_deref()
            .is_some_and(|f| self.panels.board_open_on(f));
        if !open {
            self.s7.board_due = None;
            self.board_release(app);
            return;
        }
        let now = self.now();
        self.s7.board_due = Some(now + BOARD_TICK_MS);
        let rows = self.board_rows();
        let inks = [
            self.board_ink(BoardState::Working),
            self.board_ink(BoardState::Question),
            self.board_ink(BoardState::Finished),
            self.board_ink(BoardState::Failed),
        ];
        let key = Pushed {
            rows,
            current: front.clone(),
            inks,
        };
        if self.s7.board.populated
            && app.get_panel_kind() == KIND_BOARD
            && self.s7.board.pushed.as_ref() == Some(&key)
        {
            return;
        }
        let ui: Vec<BoardRowUi> = key
            .rows
            .iter()
            .map(|r| BoardRowUi {
                tab: r.tab.as_str().into(),
                name: r.name.as_str().into(),
                agent: r.agent.into(),
                state: r.state.mark(),
                ink: self.board_ink(r.state),
                folder: r.folder.as_str().into(),
                status: r.status.as_str().into(),
                detail: r.detail.as_str().into(),
                current: front.as_deref() == Some(r.tab.as_str()),
            })
            .collect();
        let asking = key
            .rows
            .iter()
            .filter(|r| r.state == BoardState::Question)
            .count();
        let sub = match (key.rows.len(), asking) {
            (0, _) => String::new(),
            (1, 0) => "1 agent".to_string(),
            (n, 0) => format!("{n} agents"),
            (1, _) => "1 agent, asking".to_string(),
            (n, a) => format!("{n} agents, {a} asking"),
        };
        let note = if key.rows.is_empty() {
            "No agent is running in your tabs."
        } else {
            ""
        };
        app.set_panel_kind(KIND_BOARD);
        app.set_panel_title("Agents".into());
        app.set_panel_sub(sub.into());
        app.set_panel_note(note.into());
        app.set_panel_picture(slint::Image::default());
        app.set_panel_alpha(false);
        app.set_panel_actual(false);
        app.set_panel_lines(ModelRc::default());
        app.set_panel_max_cols(0);
        app.set_panel_foot("".into());
        app.set_panel_board_rows(ModelRc::from(Rc::new(VecModel::from(ui))));
        self.s7.board.populated = true;
        self.s7.board.pushed = Some(key);
    }

    /// The window shows no board: its rows go, and the panel kind goes back to nothing when it was the board's.
    fn board_release(&mut self, app: &AppWindow) {
        self.s7.board.pushed = None;
        if !self.s7.board.populated {
            return;
        }
        self.s7.board.populated = false;
        app.set_panel_board_rows(ModelRc::default());
        if app.get_panel_kind() == KIND_BOARD {
            app.set_panel_kind(0);
            app.set_panel_title("".into());
            app.set_panel_sub("".into());
            app.set_panel_note("".into());
        }
    }

    /// A row of the board was clicked: that tab comes to the front.
    pub(super) fn board_pick(&mut self, tab: &str) {
        if !self.is_shell_tab(tab) {
            return;
        }
        self.model.pick(tab);
        self.refresh();
        self.focus_terminal();
    }

    /// The rows the terminal's menu gains while some tab has an agent: one to show or hide the board.
    pub(crate) fn board_menu_rows(&self, tab: &str) -> Vec<(MenuRowData, String)> {
        let any = self
            .shell_tabs()
            .iter()
            .any(|(t, _)| self.hub.agent_here(t));
        if !any && !self.panels.board_open_on(tab) {
            return Vec::new();
        }
        let label = if self.panels.board_open_on(tab) {
            BOARD_HIDE
        } else {
            BOARD_SHOW
        };
        vec![(
            MenuRowData {
                label: label.to_string(),
                hint: self.key_label_of(ChordId::Board).to_string(),
                icon: crate::menus_ui::GLYPH_OPEN,
                filled_icon: false,
                disabled: false,
                trailing_icon: None,
                trailing_filled: false,
                trailing_tip: String::new(),
            },
            String::new(),
        )]
    }

    /// A row of the board part of the terminal's menu was picked. False for any other row.
    pub(crate) fn board_menu_picked(&mut self, tab: &str, label: &str) -> bool {
        match label {
            BOARD_SHOW => {
                self.open_convo_panel(tab, PanelKind::Board);
                true
            }
            BOARD_HIDE => {
                if self.panels.close(tab) {
                    self.refresh();
                    self.panel_changed(tab);
                }
                true
            }
            _ => false,
        }
    }

    // ---- questions as notifications --------------------------------------------------------------------------------

    /// Looks at the questions the hub holds and shows the notifications the rules allow.
    pub(super) fn notify_update(&mut self) {
        let tabs = self.hub.question_tabs();
        if tabs.is_empty() && self.s7.notifier.is_idle() {
            return;
        }
        let asking: Vec<Asking> = tabs
            .iter()
            .map(|t| Asking {
                tab: t.clone(),
                name: self
                    .view
                    .rows
                    .iter()
                    .find(|r| &r.id == t)
                    .map_or_else(String::new, |r| r.label.clone()),
                agent: agent_name(self.hub.agent_kind(t)).to_string(),
                text: self.hub.question_text(t),
            })
            .collect();
        let front = self.model.active_shell().map(|t| t.id.clone());
        let look = Look {
            now: self.now(),
            enabled: notify_ask_on(&self.prefs),
            front: front.as_deref(),
            focused: self.focused && !self.minimised,
        };
        let toasts = self.s7.notifier.update(&look, &asking);
        for toast in toasts {
            self.show_toast(&toast);
        }
    }

    fn show_toast(&mut self, toast: &Toast) {
        self.s7.toasts += 1;
        crate::diag_ctl::crumb_often("toast", &[("tab", crate::diag_ctl::txt(&toast.tab))]);
        match &self.s7.toast {
            Some(sink) => sink(toast),
            None => (self.flash)(),
        }
    }

    /// The person clicked a notification: the window comes up and the tab it was about comes to the front.
    pub(super) fn on_toast_clicked(&mut self, tab: &str) {
        (self.deps.raise)();
        self.front_tab_of_toast(tab);
    }

    /// The tab a notification was about comes to the front, if it is still there.
    fn front_tab_of_toast(&mut self, tab: &str) {
        if self.is_shell_tab(tab) {
            self.model.pick(tab);
            self.refresh();
            self.focus_terminal();
        }
    }

    /// The test hook's stand-in for a click on a notification.
    /// It does not raise the window: a run of the suite never takes the foreground.
    pub fn e2e_toast_click(&mut self, tab: &str) {
        self.front_tab_of_toast(tab);
    }

    /// Windows would not take the notification (no shortcut with this app's id, as in a build run from a folder): the
    /// taskbar button flashes instead, if nobody is looking.
    pub(super) fn on_toast_failed(&mut self) {
        (self.flash)();
    }

    // ---- the timer ------------------------------------------------------------------------------------------------

    /// When stage 7 next wants the timer: the next port look, the end of a question's grace, the open board's clock.
    pub(crate) fn surf7_next_due(&self) -> Option<u64> {
        [
            self.s7.dev.next_due(),
            self.s7.notifier.next_due(),
            self.s7.board_due,
            self.s7.dev_retry.values().copied().min(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The timer's turn: port looks, grace endings, the board's clocks.
    pub(super) fn surf7_tick(&mut self, now: u64) {
        let due: Vec<String> = self
            .s7
            .dev_retry
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(t, _)| t.clone())
            .collect();
        for tab in due {
            self.s7.dev_retry.remove(&tab);
            self.dev_read(&tab);
        }
        self.dev_pump();
        if self.s7.notifier.next_due().is_some_and(|d| d <= now) {
            self.notify_update();
        }
        if self.s7.board_due.is_some_and(|d| d <= now)
            && let Some(app) = self.app()
        {
            self.sync_board(&app);
        }
    }

    /// The e2e probe's view of stage 7.
    pub(crate) fn surf7_probe(&self) -> serde_json::Value {
        let front = self.front_id();
        let chip = front
            .as_deref()
            .and_then(|f| self.s7.dev.shown(f))
            .map(|c| serde_json::json!({"url": c.url, "port": c.port}));
        serde_json::json!({
            "chip": chip,
            "chip_label": self.app().map(|a| a.get_preview_chip_label().to_string()),
            "tracked": self.s7.dev.tracked(),
            "tracked_urls": self.s7.dev.tracked_urls(),
            "toasts": self.s7.toasts,
            "notify_waiting": self.s7.notifier.waiting(),
            "board_open": front.as_deref().is_some_and(|f| self.panels.board_open_on(f)),
            "board_timer": self.s7.board_due.is_some(),
            "board_rows": self.board_rows().iter().map(|r| serde_json::json!({
                "tab": r.tab, "name": r.name, "agent": r.agent, "state": r.state.word(),
                "folder": r.folder, "status": r.status, "detail": r.detail,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Installs Windows' notifications and the system's listening table for the real window, or under `--e2e` the
/// recorder (nothing reaches the desktop).
pub fn install(rc: &Rc<RefCell<AppCtl>>, post: Post, e2e: bool) {
    let mut c = rc.borrow_mut();
    c.set_port_probe(Arc::new(flare_win::tcp_owner::is_listening));
    if e2e {
        c.set_toast_sink(Rc::new(|t: &Toast| {
            crate::e2e_probes::surf7::record_toast(&t.tab, &t.title, &t.body);
        }));
        return;
    }
    c.set_toast_sink(Rc::new(move |t: &Toast| {
        let req = flare_win::toast::ToastRequest {
            tab: t.tab.clone(),
            title: t.title.clone(),
            body: t.body.clone(),
        };
        let clicked = Arc::clone(&post);
        let failed = Arc::clone(&post);
        flare_win::toast::show_async(
            flare_win::names::AUMID,
            req,
            Arc::new(move |tab| clicked(UiMsg::ToastClicked(tab))),
            Arc::new(move |r| {
                if let Err(e) = r {
                    log::warn!("the notification was not accepted: {e}");
                    failed(UiMsg::ToastFailed);
                }
            }),
        );
    }));
}
