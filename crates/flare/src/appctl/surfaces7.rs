//! Live preview and question notifications, in the window (Surfaces stage 7).
//!
//! The rules are `flare_surfaces::{devpreview, notify}` (pure, with their tests) and
//! `flare_term::dev_server` (what a program printed). This file is the part that needs `AppCtl`'s state: the tabs, the
//! hub, the panels, the focus and the timer.
//!
//! - **Preview chip.** The terminal reads local server addresses off its own screen, in any tab. After a tab prints,
//!   the window asks its handle what it read (the terminal looks at its screen at most every 40 ms); the chip waits for a
//!   listener on that port (a worker looks in the system's listening table, never connecting to it), and goes when
//!   nothing listens or the screen was cleared. A remote tab's address is its host's (rule 1213): its chip shows as
//!   soon as the screen names it, and a click opens an ssh tunnel to the host (`remote_tunnel`) and the page through
//!   it; the tunnel is kept for the tab and port, and closes when the chip goes or the tab closes.
//! - **Notifications.** On each look at the hub the notifier decides which questions get a toast; a toast goes to the
//!   sink the window installed (Windows' own, or under `--e2e` a recording one).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use flare_settings::agent_prefs::notify_ask_on;
use flare_settings::term_prefs::dev_preview;
use flare_surfaces::devpreview::{ChipServer, DevChips, PortLook};
use flare_surfaces::notify::{Asking, Look, Notifier, Toast};
use flare_term::dev_server::{self, LocalServer};

use super::{AppCtl, Post, UiMsg};

/// How soon a screen the terminal has not read yet is asked about again.
pub const DEV_RETRY_MS: u64 = 50;

/// How often a remote tab's open tunnels are asked whether their ssh still runs (rule 1214).
pub const TUNNEL_CHECK_MS: u64 = 1000;

/// Looks whether anything listens on a local port. Production reads the system's listening table; the tests hand a
/// table of their own.
///
/// Called as `(port, shell pid)`: true only when the port has a listener AND every listener on it belongs to the
/// tab's own process tree (the shell and what it started). A port another program holds is no chip, and says so
/// ([`PortLook::Foreign`]) so the chip goes at once.
pub type PortProbe = Arc<dyn Fn(u16, u32) -> PortLook + Send + Sync>;
/// Shows a notification. Production asks Windows; under `--e2e` the sink only records.
pub type ToastSink = Rc<dyn Fn(&Toast)>;

/// What the window holds for stage 7.
pub(super) struct Surf7 {
    pub dev: DevChips,
    pub probe: PortProbe,
    pub notifier: Notifier,
    pub toast: Option<ToastSink>,
    /// How many notifications were handed to the sink (the e2e and the tests read it).
    pub toasts: u64,
    /// The previews switch as last seen, so a change of it is noticed once.
    pub dev_on: bool,
    /// Tabs whose screen has output the terminal has not read yet (asked again too soon), and when to ask each
    /// again. One deadline per tab: another tab's output never pushes it back.
    pub dev_retry: std::collections::HashMap<String, u64>,
    /// Rule 1213: the tunnels of remote tabs' chips, by tab and host port, and the ones being opened; how they open.
    pub tunnels: std::collections::HashMap<(String, u16), crate::remote_tunnel::Tunnel>,
    pub tunnels_opening: std::collections::HashSet<(String, u16)>,
    pub tunnel_opener: crate::remote_tunnel::TunnelOpener,
    /// Rule 1214: when the open tunnels are next asked whether they still carry (only while there are any).
    pub tunnel_check_at: u64,
}

impl Surf7 {
    pub fn new() -> Self {
        Self {
            dev: DevChips::new(),
            probe: Arc::new(|_, _| PortLook::Nobody),
            notifier: Notifier::new(),
            toast: None,
            toasts: 0,
            dev_on: true,
            dev_retry: std::collections::HashMap::new(),
            tunnels: std::collections::HashMap::new(),
            tunnels_opening: std::collections::HashSet::new(),
            // No tunnel until the window installs one: a test never starts ssh.
            tunnel_opener: Arc::new(|_| Err("no tunnel here".to_string())),
            tunnel_check_at: 0,
        }
    }
}

/// The agent's name as a notification says it.
fn agent_name(k: Option<flare_agent::kinds::DetectedAgent>) -> &'static str {
    use flare_agent::kinds::DetectedAgent;
    match k {
        Some(DetectedAgent::Claude) => "Claude",
        Some(DetectedAgent::Codex) => "Codex",
        Some(DetectedAgent::Agy) => "Antigravity",
        _ => "Agent",
    }
}

/// "localhost:5173" for the chip: the origin without its scheme. The chip only ever holds an origin
/// (`scheme://host:port/`), so this is exactly what a click opens.
pub fn chip_label(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// A chip's address, as the web panel would take it. None for anything the panel would not.
fn chip_server(s: &LocalServer) -> Option<ChipServer> {
    // The chip opens the ORIGIN only: whatever path or query the program printed is dropped.
    let local = dev_server::parse_local(&s.url)?;
    let url = flare_surfaces::web::normalize_url(&local.url).ok()?;
    // The same rule again on the normal form: the chip only ever offers a local origin.
    let again = dev_server::parse_local(&url)?;
    if again.url != local.url {
        return None;
    }
    Some(ChipServer {
        url: again.url,
        port: again.port,
    })
}

impl AppCtl {
    /// How the window finds out whether a local port has a listener (startup; the tests hand a stand-in).
    pub fn set_port_probe(&mut self, probe: PortProbe) {
        self.s7.probe = probe;
    }

    /// The pids of the live shells (the tests read it).
    #[cfg(test)]
    pub(crate) fn deps_shell_pids(&self) -> Vec<u32> {
        self.deps
            .host
            .manager()
            .live_pids()
            .into_iter()
            .map(|(_, p)| p)
            .collect()
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
        // A remote tab's `localhost` is the host's: its chip opens through a tunnel to the host (rule 1213).
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
        // Its tunnels close with it (rule 1213).
        self.s7.tunnels.retain(|(t, _), _| t != tab);
        self.s7.tunnels_opening.retain(|(t, _)| t != tab);
    }

    /// How a remote tab's chip opens its tunnel (startup: ssh, or under `--e2e` the loopback stand-in; tests a fake).
    pub fn set_tunnel_opener(&mut self, opener: crate::remote_tunnel::TunnelOpener) {
        self.s7.tunnel_opener = opener;
    }

    /// Rule 1213: a remote tab's tunnel closes once its chip went or holds another port. Rule 1214: a tunnel whose ssh
    /// ended on its own (the link to the host died) is dropped too, so the chip shows again and a click opens a new one.
    fn drop_gone_tunnels(&mut self) {
        let dev = &self.s7.dev;
        self.s7.tunnels.retain(|(t, port), tunnel| {
            if !dev.server(t).is_some_and(|s| s.port == *port) {
                return false;
            }
            let alive = tunnel.alive();
            if !alive {
                log::info!("the preview tunnel for port {port} ended; a click opens a new one");
            }
            alive
        });
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
        let pids = self.deps.host.manager().live_pids();
        for check in self.s7.dev.due(now) {
            // A remote tab's server listens on the host, where this machine cannot look: what its screen says is
            // believed, and the chip goes when the screen is cleared (rule 1213). No port of this machine is looked at.
            if self.model.remote_of(&check.tab).is_some() {
                (self.deps.post)(UiMsg::DevLook {
                    tab: check.tab,
                    token: check.token,
                    look: PortLook::InTree,
                });
                continue;
            }
            // The tab's shell: its tree is the only place a listener is believed. No shell, no chip.
            let shell = pids
                .iter()
                .find(|(t, _)| *t == check.tab)
                .map_or(0, |(_, p)| *p);
            let probe = Arc::clone(&self.s7.probe);
            let post = Arc::clone(&self.deps.post);
            let job = move || {
                let look = probe(check.port, shell);
                post(UiMsg::DevLook {
                    tab: check.tab,
                    token: check.token,
                    look,
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
    pub(super) fn on_dev_look(&mut self, tab: &str, token: u64, look: PortLook) {
        let now = self.now();
        self.s7.dev.on_check(tab, token, look, now);
        self.dev_pump();
        self.sync_chip();
    }

    /// The chip on the window: shown for the tab in front while it holds a listening local server and the panel does not
    /// show that page already.
    pub(super) fn sync_chip(&mut self) {
        self.drop_gone_tunnels();
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
        let chip = self.chip_for_front();
        let label = chip
            .as_ref()
            .map_or_else(String::new, |c| chip_label(&c.url));
        if app.get_preview_chip_label().as_str() != label {
            app.set_preview_chip_label(label.into());
        }
        // The tooltip is the full origin a click opens.
        let tip = chip.map_or_else(String::new, |c| c.url);
        if app.get_preview_chip_tip().as_str() != tip {
            app.set_preview_chip_tip(tip.into());
        }
    }

    /// The chip the tab in front offers, if any.
    fn chip_for_front(&self) -> Option<ChipServer> {
        if !dev_preview(&self.prefs) {
            return None;
        }
        let front = self.model.active_shell()?.id.clone();
        let chip = self.s7.dev.shown(&front)?.clone();
        // The panel already shows that server (on any page of it, after the person followed a link inside the app):
        // nothing to offer, and a click must never send it back to the origin.
        // A remote tab's panel shows the page at its tunnel's port on this machine (rule 1213).
        let tunnel = self
            .s7
            .tunnels
            .get(&(front.clone(), chip.port))
            .map(|t| t.local_port);
        let shows = self
            .panels
            .web_target(&front)
            .and_then(|t| dev_server::parse_local(&t))
            .is_some_and(|s| match tunnel {
                Some(local) => s.port == local,
                None => s.url == chip.url,
            });
        if shows {
            return None;
        }
        Some(chip)
    }

    /// Moves the panel's page inside its server, as following a link does (tests).
    #[cfg(test)]
    pub(crate) fn follow_link_in_panel(&mut self, tab: &str, url: &str) {
        self.panels.set_web_url(tab, url);
        self.sync_chip();
    }

    /// The chip was clicked. The address is checked once more here (the chip may only ever open a local one), and the
    /// port is looked at AGAIN on a worker: the page opens only when that look finds every listener in the tab's own
    /// process tree, so a port that changed hands since the last look opens nothing.
    pub(super) fn preview_chip_clicked(&mut self) {
        let Some(chip) = self.chip_for_front() else {
            return;
        };
        let Some(front) = self.front_id() else { return };
        if dev_server::parse_local(&chip.url).is_none() {
            return;
        }
        let Some(token) = self.s7.dev.token(&front) else {
            return;
        };
        if self.model.remote_of(&front).is_some() {
            self.remote_chip_clicked(&front, &chip, token);
            return;
        }
        let shell = self
            .deps
            .host
            .manager()
            .live_pids()
            .iter()
            .find(|(t, _)| *t == front)
            .map_or(0, |(_, p)| *p);
        let probe = Arc::clone(&self.s7.probe);
        let post = Arc::clone(&self.deps.post);
        let port = chip.port;
        let job = move || {
            let look = probe(port, shell);
            post(UiMsg::DevClickLook {
                tab: front,
                token,
                look,
            });
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-port-click".into())
            .spawn(job)
        {
            log::warn!("could not start the port look: {e}");
        }
    }

    /// The look made for a click came back. It also feeds the chip (a foreign holder takes it at once). The page opens
    /// only for an in-tree answer about the SAME chip, still on the tab in front.
    pub(super) fn on_dev_click_look(&mut self, tab: &str, token: u64, look: PortLook) {
        // Apart from the periodic looks: only a foreign holder acts on the chip (see `on_click_check`).
        self.s7.dev.on_click_check(tab, token, look);
        self.sync_chip();
        if look != PortLook::InTree || self.s7.dev.token(tab) != Some(token) {
            return;
        }
        if self.front_id().as_deref() != Some(tab) {
            return;
        }
        let Some(chip) = self.chip_for_front() else {
            return;
        };
        if dev_server::parse_local(&chip.url).is_none() {
            return;
        }
        if self.browser_open(tab, &chip.url).is_ok() {
            self.sync_chip();
        }
    }

    /// Rule 1213: a remote tab's chip was clicked. Its tunnel, when open, is used again at once; else one is opened on a
    /// worker to the tab's machine (never twice for one tab and port), and the page opens when it comes up.
    fn remote_chip_clicked(&mut self, tab: &str, chip: &ChipServer, token: u64) {
        let key = (tab.to_string(), chip.port);
        // A tunnel whose ssh ended is never used again: it is dropped and a new one opened (rule 1214).
        if self.s7.tunnels.get(&key).is_some_and(|t| !t.alive()) {
            self.s7.tunnels.remove(&key);
        }
        if let Some(t) = self.s7.tunnels.get(&key) {
            if let Some(url) = crate::remote_tunnel::tunnel_url(&chip.url, t.local_port)
                && self.browser_open(tab, &url).is_ok()
            {
                self.sync_chip();
            }
            return;
        }
        if self.s7.tunnels_opening.contains(&key) {
            return;
        }
        let post = Arc::clone(&self.deps.post);
        let Some(machine) = self
            .model
            .remote_of(tab)
            .and_then(|tag| self.machines.iter().find(|m| m.id == tag.id))
            .cloned()
        else {
            let name = self
                .model
                .remote_of(tab)
                .map_or_else(String::new, |t| t.name.clone());
            post(UiMsg::RemoteTunnel {
                tab: tab.to_string(),
                port: chip.port,
                token,
                opened: Err(format!("{name} is not in Settings > Machines any more")),
            });
            return;
        };
        self.s7.tunnels_opening.insert(key);
        let opener = Arc::clone(&self.s7.tunnel_opener);
        let ask = crate::remote_tunnel::TunnelAsk {
            machine,
            host_port: chip.port,
        };
        let (id, port) = (tab.to_string(), chip.port);
        let job = move || {
            let opened = opener(&ask);
            post(UiMsg::RemoteTunnel {
                tab: id,
                port,
                token,
                opened,
            });
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-tunnel-open".into())
            .spawn(job)
        {
            log::warn!("could not start the tunnel: {e}");
            self.s7.tunnels_opening.remove(&(tab.to_string(), port));
        }
    }

    /// Rule 1213: a remote tab's tunnel came up (or did not). It is kept only while the same chip stands on that tab;
    /// the page opens when the tab is still in front. A failure is one line in the tab's panel.
    pub(super) fn on_remote_tunnel(
        &mut self,
        tab: &str,
        port: u16,
        token: u64,
        opened: Result<crate::remote_tunnel::Tunnel, String>,
    ) {
        self.s7.tunnels_opening.remove(&(tab.to_string(), port));
        let tunnel = match opened {
            Ok(t) => t,
            Err(why) => {
                let text = format!("The preview could not be opened: {why}");
                self.say_in_panel(tab, &text, &text, "Not done on the other machine");
                return;
            }
        };
        let same = self.s7.dev.token(tab) == Some(token)
            && self.s7.dev.server(tab).is_some_and(|s| s.port == port);
        if !same {
            // The chip went meanwhile: the tunnel closes as it drops.
            return;
        }
        let local = tunnel.local_port;
        self.s7.tunnels.insert((tab.to_string(), port), tunnel);
        // From now on the tunnels are asked, on the timer, whether they still carry (rule 1214).
        self.s7.tunnel_check_at = self.now() + TUNNEL_CHECK_MS;
        if self.front_id().as_deref() != Some(tab) {
            return;
        }
        let Some(chip) = self.chip_for_front() else {
            return;
        };
        if let Some(url) = crate::remote_tunnel::tunnel_url(&chip.url, local)
            && self.browser_open(tab, &url).is_ok()
        {
            self.sync_chip();
        }
    }

    /// The tunnels open now, as (tab, host port, local port) (the tests and the e2e probe).
    pub(crate) fn remote_tunnels(&self) -> Vec<(String, u16, u16)> {
        let mut v: Vec<(String, u16, u16)> = self
            .s7
            .tunnels
            .iter()
            .map(|((t, p), x)| (t.clone(), *p, x.local_port))
            .collect();
        v.sort();
        v
    }

    // ---- questions as notifications --------------------------------------------------------------------------------

    /// Looks at the questions the hub holds and shows the notifications the rules allow.
    pub(super) fn notify_update(&mut self) {
        let mut tabs = self.hub.question_tabs();
        // Rule 295: a tab a remote viewer follows asks on the viewer's screen; this host says nothing of it, so one
        // question is one notification across both machines.
        if let Some(host) = &self.remote_host {
            tabs.retain(|t| !host.is_followed(t));
        }
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

    /// When stage 7 next wants the timer: the next port look, the end of a question's grace,
    pub(crate) fn surf7_next_due(&self) -> Option<u64> {
        [
            self.s7.dev.next_due(),
            self.s7.notifier.next_due(),
            self.s7.dev_retry.values().copied().min(),
            (!self.s7.tunnels.is_empty()).then_some(self.s7.tunnel_check_at),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The timer's turn: port looks and grace endings.
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
        // Rule 1214: a tunnel whose ssh ended on its own goes now, so the chip shows again without any new output.
        if !self.s7.tunnels.is_empty() && self.s7.tunnel_check_at <= now {
            self.s7.tunnel_check_at = now + TUNNEL_CHECK_MS;
            if self.s7.tunnels.values().any(|t| !t.alive()) {
                self.sync_chip();
            }
        }
        if self.s7.notifier.next_due().is_some_and(|d| d <= now) {
            self.notify_update();
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
            "chip_tip": self.app().map(|a| a.get_preview_chip_tip().to_string()),
            "tracked": self.s7.dev.tracked(),
            "tracked_urls": self.s7.dev.tracked_urls(),
            "tunnels": self.remote_tunnels(),
            "toasts": self.s7.toasts,
            "notify_waiting": self.s7.notifier.waiting(),
        })
    }
}

/// The once-a-run warning that the system cannot show notifications (macOS outside the signed `Flare.app`).
static UNAVAILABLE_LOGGED: flare_win::toast::FirstTime = flare_win::toast::FirstTime::new();

/// Installs Windows' notifications and the system's listening table for the real window, or under `--e2e` the
/// recorder (nothing reaches the desktop).
pub fn install(rc: &Rc<RefCell<AppCtl>>, post: Post, e2e: bool) {
    let mut c = rc.borrow_mut();
    // A remote tab's chip opens a tunnel to its host (rule 1213): ssh, or under `--e2e` the loopback stand-in, never
    // a real ssh.
    c.set_tunnel_opener(if e2e {
        Arc::new(crate::remote_tunnel::open_loopback)
    } else {
        Arc::new(|ask| crate::remote_tunnel::open_ssh(ask, crate::remote_tunnel::TUNNEL_WAIT))
    });
    c.set_port_probe(Arc::new(|port, shell| {
        use flare_win::tcp_owner::TreeLook;
        if shell == 0 {
            return PortLook::Nobody;
        }
        match flare_win::tcp_owner::tree_look(port, shell) {
            TreeLook::InTree => PortLook::InTree,
            TreeLook::Foreign => PortLook::Foreign,
            // Unknown is never trusted, and is not proof of a stranger either: one miss.
            TreeLook::Nobody | TreeLook::Unreadable => PortLook::Nobody,
        }
    }));
    if e2e {
        c.set_toast_sink(Rc::new(|t: &Toast| {
            crate::e2e_probes::surf7::record_toast(&t.tab, &t.title, &t.body);
        }));
        return;
    }
    flare_win::toast::warm_up();
    c.set_toast_sink(Rc::new(move |t: &Toast| {
        // macOS outside the signed `Flare.app` (a cargo run): the system cannot take the request, so no worker is
        // started to fail; the fallback (the Dock bounce) runs at once and the reason is logged once a run. In the
        // signed app the worker asks `UNUserNotificationCenter` (the permission once, lazily; rule 1044). The answer
        // is read on a worker at install (`warm_up`): until it is known, the worker reads it and answers itself.
        if let Some(why) = flare_win::toast::known_unavailable() {
            if UNAVAILABLE_LOGGED.take() {
                log::warn!("the notification was not accepted: {why}");
            }
            post(UiMsg::ToastFailed);
            return;
        }
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
